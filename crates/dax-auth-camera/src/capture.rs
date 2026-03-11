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
use crate::{CameraDevice, CameraError, Frame};

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
    /// Preferred format order: YUYV → MJPEG. The first supported format is
    /// selected. The driver may silently adjust the resolution; the actual
    /// negotiated dimensions are stored internally and reflected in returned
    /// [`Frame`] metadata.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::V4l2`] if the device cannot be opened, or
    /// [`CameraError::UnsupportedFormat`] if neither YUYV nor MJPEG are
    /// supported by the device.
    pub fn open(device: CameraDevice) -> Result<Self, CameraError> {
        debug!(path = %device.path, "opening V4L2 device");

        let inner = v4l::Device::with_path(&device.path)
            .map_err(|e| CameraError::V4l2(e.to_string()))?;

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
/// Preference order: YUYV → MJPEG.
///
/// Returns the selected [`PixelFormat`] together with the [`v4l::Format`]
/// that the driver actually accepted (which may differ in resolution).
fn negotiate_format(
    dev: &v4l::Device,
    device: &CameraDevice,
) -> Result<(PixelFormat, v4l::format::Format), CameraError> {
    let candidates = [
        (PixelFormat::Yuyv, v4l::FourCC::new(b"YUYV")),
        (PixelFormat::Mjpeg, v4l::FourCC::new(b"MJPG")),
    ];

    for (pixel_fmt, fourcc) in candidates {
        let desired = v4l::format::Format::new(device.width, device.height, fourcc);
        match dev.set_format(&desired) {
            Ok(actual) => {
                // Verify the driver accepted the requested FourCC.
                if actual.fourcc == fourcc {
                    return Ok((pixel_fmt, actual));
                }
                // Driver accepted a different format — keep trying.
                debug!(
                    requested = ?pixel_fmt,
                    actual_fourcc = %actual.fourcc,
                    "driver substituted a different format, continuing negotiation"
                );
            }
            Err(e) => {
                debug!(
                    format = ?pixel_fmt,
                    error = %e,
                    "format not accepted by driver, trying next"
                );
            }
        }
    }

    Err(CameraError::UnsupportedFormat {
        format: "YUYV / MJPEG".into(),
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
        use crate::CameraDevice;
        use crate::device::CameraKind;

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
