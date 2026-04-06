//! Camera capture session management.
//!
//! Implements V4L2 MMAP streaming for single-frame acquisition. Each call to
//! [`CameraCapture::capture_frame`] opens a fresh MMAP stream, grabs one frame,
//! and closes the stream. This avoids self-referential lifetime issues at the
//! cost of slightly higher per-frame overhead — acceptable for Phase 1.
//!
//! Phase 2 will introduce persistent streams with a refactored ownership model.

use std::time::Duration;

use tracing::{debug, info};
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;

use crate::frame::PixelFormat;
use crate::{CameraDevice, CameraError, CameraKind, Frame};

/// Maximum number of warm-up frames to discard when `capture_best_frame` is
/// looking for a non-black frame.  Prevents an infinite loop on broken hardware.
const DEFAULT_MAX_FRAMES: u32 = 30;

/// Number of MMAP buffers to allocate per capture stream.
///
/// Four buffers is the conventional minimum for smooth V4L2 streaming.
const MMAP_BUFFER_COUNT: u32 = 4;

/// Timeout applied to each frame read via `poll(2)`.
///
/// If the driver does not produce a buffer within this window the stream
/// returns an `io::ErrorKind::TimedOut` error, which we map to
/// [`CameraError::Timeout`].
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// An active camera capture session.
///
/// Wraps a V4L2 device handle and the negotiated format. Each call to
/// [`CameraCapture::capture_frame`] creates a fresh MMAP stream so that
/// frame data can be returned as an owned `Vec<u8>` without lifetime
/// complications.
pub struct CameraCapture {
    /// The camera device metadata (path, kind, negotiated resolution).
    pub(crate) device: CameraDevice,
    /// The open V4L2 device handle.
    inner: v4l::Device,
    /// The negotiated pixel format.
    pub format: PixelFormat,
    /// Width reported by the driver after format negotiation.
    pub width: u32,
    /// Height reported by the driver after format negotiation.
    pub height: u32,
}

impl CameraCapture {
    /// Open a camera device and negotiate a capture format.
    ///
    /// Preferred format order depends on camera kind:
    /// - RGB: YUYV → MJPEG → BGR24
    /// - Infrared: GREY → Y16 → YUYV → MJPEG
    /// - RGB+IR: YUYV → MJPEG → BGR24 → GREY → Y16
    ///
    /// The first supported format is selected. The driver may silently adjust
    /// the resolution; the actual negotiated dimensions are stored internally
    /// and reflected in returned [`Frame`] metadata.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::V4l2`] if the device cannot be opened, or
    /// [`CameraError::UnsupportedFormat`] if neither YUYV nor MJPEG are
    /// supported by the device.
    pub fn open(device: CameraDevice) -> Result<Self, CameraError> {
        debug!(path = %device.path, "opening V4L2 device");

        let inner =
            v4l::Device::with_path(&device.path).map_err(|e| CameraError::V4l2(e.to_string()))?;

        // Negotiate format: prefer YUYV, fall back to MJPEG.
        let (format, actual_fmt) = negotiate_format(&inner, &device)?;

        let width = actual_fmt.width;
        let height = actual_fmt.height;

        info!(
            path = %device.path,
            format = ?format,
            width,
            height,
            "camera format negotiated"
        );

        Ok(Self {
            device,
            inner,
            format,
            width,
            height,
        })
    }

    /// Capture a single raw frame from the camera, blocking until available.
    ///
    /// Opens a fresh MMAP stream, grabs one frame with a 5-second timeout,
    /// copies the buffer into an owned `Vec<u8>`, and closes the stream.
    ///
    /// The returned [`Frame`]'s pixel data is zeroed on drop via
    /// [`zeroize::ZeroizeOnDrop`].
    ///
    /// # Errors
    ///
    /// - [`CameraError::Timeout`] — driver did not produce a buffer within 5 s
    /// - [`CameraError::CaptureFailed`] — any other V4L2 / MMAP error
    pub fn capture_frame(&mut self) -> Result<Frame, CameraError> {
        // Create a fresh MMAP stream for this single-frame capture.
        let mut stream = Stream::with_buffers(&self.inner, Type::VideoCapture, MMAP_BUFFER_COUNT)
            .map_err(|e| CameraError::CaptureFailed(e.to_string()))?;

        stream.set_timeout(FRAME_TIMEOUT);

        let (frame_data, _meta) = stream.next().map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                CameraError::Timeout
            } else {
                CameraError::CaptureFailed(e.to_string())
            }
        })?;

        // Copy out of the MMAP buffer before closing the stream.
        let data = frame_data.to_vec();

        Ok(Frame {
            data,
            width: self.width,
            height: self.height,
            kind: self.device.kind,
            format: self.format,
        })
    }

    /// Async wrapper for [`CameraCapture::capture_frame`].
    ///
    /// Runs the blocking V4L2 call inside [`tokio::task::block_in_place`],
    /// which yields the current thread to the tokio scheduler while blocking.
    /// Requires the multi-thread tokio runtime (used by `dax-authd`).
    ///
    /// # Errors
    ///
    /// Propagates errors from [`CameraCapture::capture_frame`].
    pub async fn capture_frame_async(&mut self) -> Result<Frame, CameraError> {
        tokio::task::block_in_place(|| self.capture_frame())
    }

    /// Capture the best available frame, waiting for auto-exposure to stabilise.
    ///
    /// UVC cameras (including integrated webcams) output dark frames while the
    /// sensor's auto-exposure and auto-gain circuits settle after the stream is
    /// opened. Empirically this takes ~10 frames at 30 fps (~330 ms).
    ///
    /// This method discards frames until the average luminance exceeds
    /// `MIN_LUMA_THRESHOLD` (40/255 ≈ 16%), then returns the first frame above
    /// that threshold. If no bright-enough frame appears within `max_frames`,
    /// returns the least-dark frame captured (best effort).
    ///
    /// # Errors
    ///
    /// - [`CameraError::NoUsableFrame`] — every frame was too dark (device may
    ///   be covered or the environment may have no light)
    /// - Any error from [`CameraCapture::capture_frame_async`]
    pub async fn capture_best_frame(&mut self) -> Result<Frame, CameraError> {
        /// Average luma threshold (0–255).  Frames below this are discarded as
        /// auto-exposure warm-up artefacts.  Value chosen from empirical
        /// measurement: ASUS FHD UVC camera settles from luma ~18 → ~89 over
        /// 10 frames; a threshold of 40 reliably rejects the dark warm-up
        /// frames while accepting all well-lit frames.
        const MIN_LUMA_THRESHOLD: u8 = 40;

        let max_frames = DEFAULT_MAX_FRAMES;

        let mut best_frame: Option<Frame> = None;
        let mut best_luma: u8 = 0;

        for attempt in 1..=max_frames {
            let frame = self.capture_frame_async().await?;

            // Compute average luma from JPEG/raw data.
            // For MJPEG this is a rough estimate on the compressed stream;
            // for raw formats it is exact.  Accurate enough for threshold use.
            let avg_luma = estimate_frame_luma(&frame);

            debug!(
                attempt,
                avg_luma,
                threshold = MIN_LUMA_THRESHOLD,
                "auto-exposure check"
            );

            if avg_luma >= MIN_LUMA_THRESHOLD {
                debug!(attempt, avg_luma, "captured usable frame (luma OK)");
                return Ok(frame);
            }

            // Keep the brightest frame seen so far as a fallback.
            if avg_luma > best_luma {
                best_luma = avg_luma;
                best_frame = Some(frame);
            }

            debug!(
                attempt,
                avg_luma, "frame too dark (auto-exposure settling), retrying"
            );
        }

        // If we never reached the threshold, return the best frame we got.
        // This handles very dark environments where the camera has settled but
        // the scene is genuinely dim — better to attempt detection than to fail.
        if let Some(frame) = best_frame {
            info!(
                best_luma,
                threshold = MIN_LUMA_THRESHOLD,
                "auto-exposure did not reach threshold after {max_frames} frames; \
                 using best available frame"
            );
            return Ok(frame);
        }

        Err(CameraError::NoUsableFrame {
            attempts: max_frames,
        })
    }

    /// Stop streaming and release V4L2 resources.
    ///
    /// In Phase 1 this is a no-op: the device handle is dropped via RAII when
    /// `CameraCapture` itself is dropped. This method exists for callers that
    /// want to make the intent explicit.
    pub fn stop(self) {
        // Drop releases `self.inner` (the v4l::Device).
        drop(self);
    }
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Estimate the average luminance of a frame as a value in `[0, 255]`.
///
/// For MJPEG: samples the raw compressed bytes as a proxy (underestimates
/// true luma but is fast and monotonically related to actual brightness).
/// For raw formats (YUYV, GREY, Y16, BGR24): computes the true average luma.
fn estimate_frame_luma(frame: &Frame) -> u8 {
    if frame.data.is_empty() {
        return 0;
    }

    let sum: u64 = match frame.format {
        // GREY: every byte is a luma sample.
        crate::frame::PixelFormat::Grey => frame.data.iter().map(|&b| b as u64).sum(),
        // Y16 LE: high byte is the useful luma.
        crate::frame::PixelFormat::Y16 => frame.data.chunks_exact(2).map(|c| c[1] as u64).sum(),
        // YUYV: Y bytes are at even indices (0, 2, 4, ...).
        crate::frame::PixelFormat::Yuyv => frame
            .data
            .iter()
            .step_by(2)
            .map(|&b| b as u64)
            .sum::<u64>()
            .saturating_mul(2), // adjust to full-pixel count denominator below
        // BGR24: approximate luma = 0.114R + 0.587G + 0.299B ≈ (B+G+R)/3
        crate::frame::PixelFormat::Bgr24 => frame
            .data
            .chunks_exact(3)
            .map(|c| {
                let (b, g, r) = (c[0] as u32, c[1] as u32, c[2] as u32);
                ((3 * b + 6 * g + r) / 10) as u64
            })
            .sum(),
        // MJPEG: sample raw bytes as proxy.  JPEG entropy-coded bytes are
        // spread through a wide range, but very dark frames produce much
        // smaller files with lower average byte values.
        crate::frame::PixelFormat::Mjpeg => frame.data.iter().map(|&b| b as u64).sum(),
    };

    let n = frame.data.len() as u64;
    (sum.saturating_div(n).min(255)) as u8
}

/// Try to negotiate a pixel format with the V4L2 driver.
///
/// Preference order depends on camera kind and is tried at multiple resolutions.
///
/// We attempt each format at decreasing resolutions: the device's reported
/// best resolution, then 1280×720, then 640×480. This is necessary because
/// some webcams (e.g. Sonix) only accept YUYV at ≤640×480 while MJPEG works
/// at higher resolutions. Taking the device's max resolution and requesting
/// YUYV at that resolution causes the driver to silently substitute MJPEG.
///
/// Returns the selected [`PixelFormat`] together with the [`v4l::Format`]
/// that the driver actually accepted (which may differ in resolution).
fn negotiate_format(
    dev: &v4l::Device,
    device: &CameraDevice,
) -> Result<(PixelFormat, v4l::format::Format), CameraError> {
    // Format priority rationale:
    //
    // For RGB cameras on USB, MJPEG is ALWAYS preferred over YUYV because:
    //   - YUYV at 1920×1080 saturates USB 2.0 bandwidth (~148 MB/s raw vs ~60 MB/s bus)
    //     which causes the V4L2 driver to drop/stall and the first stream.next() to block
    //     indefinitely instead of returning with a timeout.
    //   - MJPEG compresses on-device and stays well within USB bandwidth at all resolutions.
    //   - Both are decoded to RGB in software; quality for 640×640 SCRFD inference is identical.
    //
    // For IR cameras, GREY/Y16 are native hardware formats and do not have bandwidth issues.
    let candidates: &[PixelFormat] = match device.kind {
        CameraKind::Rgb => &[PixelFormat::Mjpeg, PixelFormat::Yuyv, PixelFormat::Bgr24],
        CameraKind::Infrared => &[
            PixelFormat::Grey,
            PixelFormat::Y16,
            PixelFormat::Mjpeg,
            PixelFormat::Yuyv,
        ],
        CameraKind::RgbAndInfrared => &[
            PixelFormat::Mjpeg,
            PixelFormat::Yuyv,
            PixelFormat::Bgr24,
            PixelFormat::Grey,
            PixelFormat::Y16,
        ],
    };

    // Resolution strategy:
    //   - Cap at 640×480 for capture. SCRFD inference runs at 640×640 internally anyway;
    //     feeding 1920×1080 wastes USB bandwidth and memory with zero accuracy benefit.
    //   - Always include 640×480 and 640×360 as guaranteed-safe fallbacks.
    let capped_w = device.width.min(640);
    let capped_h = device.height.min(480);
    let resolutions: &[(u32, u32)] = if capped_w == 640 && capped_h == 480 {
        &[(640, 480), (640, 360)]
    } else {
        &[(capped_w, capped_h), (640, 480), (640, 360)]
    };

    for pixel_fmt in candidates {
        let fourcc = pixel_fmt.to_v4l2_fourcc();
        for &(w, h) in resolutions {
            let desired = v4l::format::Format::new(w, h, fourcc);
            match dev.set_format(&desired) {
                Ok(actual) => {
                    if actual.fourcc == fourcc {
                        debug!(
                            format = ?pixel_fmt,
                            width = actual.width,
                            height = actual.height,
                            "format negotiated successfully"
                        );
                        return Ok((*pixel_fmt, actual));
                    }
                    // Driver substituted a different format — try next resolution.
                    debug!(
                        requested = ?pixel_fmt,
                        width = w,
                        height = h,
                        actual_fourcc = %actual.fourcc,
                        "driver substituted format, trying next resolution"
                    );
                }
                Err(e) => {
                    debug!(
                        format = ?pixel_fmt,
                        width = w,
                        height = h,
                        error = %e,
                        "format/resolution rejected by driver"
                    );
                }
            }
        }
    }

    // Some drivers reject explicit set_format attempts but have a usable current
    // format already configured. Accept it as a final fallback.
    if let Ok(current) = dev.format() {
        if let Some(pf) = PixelFormat::from_v4l2_fourcc(current.fourcc) {
            debug!(
                format = ?pf,
                width = current.width,
                height = current.height,
                "using driver current format as fallback"
            );
            return Ok((pf, current));
        }
    }

    Err(CameraError::UnsupportedFormat {
        format: "YUYV / MJPEG / BGR24 / GREY / Y16".into(),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a real /dev/video* device"]
    fn open_and_capture_frame_from_real_device() {
        let device = CameraDevice::best_available().expect("need a camera to run this test");
        let mut cap = CameraCapture::open(device).expect("open failed");
        let frame = cap.capture_frame().expect("capture failed");
        assert!(frame.width > 0, "width must be positive");
        assert!(frame.height > 0, "height must be positive");
        assert!(!frame.data.is_empty(), "frame data must not be empty");
    }

    #[tokio::test]
    #[ignore = "requires a real /dev/video* device and multi-thread tokio runtime"]
    async fn capture_best_frame_returns_non_black_frame() {
        let device = CameraDevice::best_available().expect("need a camera");
        let mut cap = CameraCapture::open(device).expect("open failed");
        let frame = cap.capture_best_frame().await.expect("no usable frame");
        assert!(
            frame.data.iter().any(|&b| b != 0),
            "best frame should not be all-black"
        );
    }

    #[test]
    fn open_returns_error_for_nonexistent_device() {
        use crate::device::CameraKind;
        use crate::CameraDevice;

        let fake_device = CameraDevice {
            path: "/dev/video_does_not_exist_99".into(),
            name: "fake".into(),
            kind: CameraKind::Rgb,
            width: 640,
            height: 480,
        };

        let result = CameraCapture::open(fake_device);
        assert!(
            result.is_err(),
            "opening a nonexistent device must return an error"
        );
    }
}
