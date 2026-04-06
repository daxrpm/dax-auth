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
    /// The negotiated pixel format (YUYV preferred, MJPEG fallback).
    format: PixelFormat,
    /// Width reported by the driver after format negotiation.
    width: u32,
    /// Height reported by the driver after format negotiation.
    height: u32,
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

    /// Capture the best available frame, skipping all-black warm-up frames.
    ///
    /// Attempts up to `max_frames` captures, returning the first frame whose
    /// `data` buffer contains at least one non-zero byte (i.e. not all-black).
    /// If every frame is all-black, returns [`CameraError::NoUsableFrame`].
    ///
    /// This is important because many cameras output several black frames while
    /// the sensor is initialising (auto-exposure, gain settling).
    ///
    /// # Errors
    ///
    /// - [`CameraError::NoUsableFrame`] — all captured frames were all-black
    /// - Any error from [`CameraCapture::capture_frame_async`]
    pub async fn capture_best_frame(&mut self) -> Result<Frame, CameraError> {
        let max_frames = DEFAULT_MAX_FRAMES;

        for attempt in 1..=max_frames {
            let frame = self.capture_frame_async().await?;

            if frame.data.iter().any(|&b| b != 0) {
                debug!(attempt, "captured usable frame");
                return Ok(frame);
            }

            debug!(attempt, "frame is all-black, retrying");
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
    let candidates: &[PixelFormat] = match device.kind {
        CameraKind::Rgb => &[PixelFormat::Yuyv, PixelFormat::Mjpeg, PixelFormat::Bgr24],
        CameraKind::Infrared => &[
            PixelFormat::Grey,
            PixelFormat::Y16,
            PixelFormat::Yuyv,
            PixelFormat::Mjpeg,
        ],
        CameraKind::RgbAndInfrared => &[
            PixelFormat::Yuyv,
            PixelFormat::Mjpeg,
            PixelFormat::Bgr24,
            PixelFormat::Grey,
            PixelFormat::Y16,
        ],
    };

    // Resolutions to try, in order of preference.
    // 640×480 is universally supported and more than sufficient for 112×112 inference.
    let resolutions: &[(u32, u32)] = &[
        (device.width, device.height), // device's reported best (may be 1920×1080)
        (1280, 720),
        (640, 480),
    ];

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
