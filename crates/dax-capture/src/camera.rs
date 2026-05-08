use dax_core::{Frame, PixelFormat};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use tracing::debug;

use crate::error::{CaptureError, CaptureResult};
use crate::ir::IrCamera;

/// A single camera device opened for capture.
///
/// `Camera` owns the underlying V4L2 stream. Dropping it closes the
/// device. The struct is `!Send` by virtue of the underlying handle,
/// which matches the operating-system expectation that a video
/// stream is driven by exactly one thread.
pub struct Camera {
    inner: nokhwa::Camera,
}

impl std::fmt::Debug for Camera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Camera").finish_non_exhaustive()
    }
}

impl Camera {
    /// Open the camera at `index`, negotiating the highest resolution
    /// the device exposes in an RGB pixel layout.
    pub fn open(index: u32) -> CaptureResult<Self> {
        let format =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
        Self::open_with(index, format, "rgb")
    }

    fn open_with(
        index: u32,
        format: RequestedFormat<'_>,
        kind: &'static str,
    ) -> CaptureResult<Self> {
        let camera = nokhwa::Camera::new(CameraIndex::Index(index), format)
            .map_err(|e| CaptureError::DeviceOpen(e.to_string()))?;
        debug!(index, kind, "camera opened");
        Ok(Self { inner: camera })
    }

    /// Open an IR camera at `index`.
    ///
    /// Uses a direct V4L2 path because `nokhwa 0.10` cannot negotiate
    /// the `GREY` `FourCC` that Windows-Hello-class infrared sensors
    /// expose; the dedicated [`IrCamera`] is therefore returned.
    pub fn open_ir(index: u32) -> CaptureResult<IrCamera> {
        IrCamera::open(index)
    }

    /// Capture a single frame, decoded into packed 8-bit RGB.
    pub fn capture(&mut self) -> CaptureResult<Frame> {
        self.inner
            .open_stream()
            .map_err(|e| CaptureError::Stream(e.to_string()))?;
        let buffer = self
            .inner
            .frame()
            .map_err(|e| CaptureError::Stream(e.to_string()))?;
        let decoded = buffer
            .decode_image::<RgbFormat>()
            .map_err(|e| CaptureError::Decode(e.to_string()))?;
        let width = decoded.width();
        let height = decoded.height();
        let bytes = decoded.into_raw();
        debug!(width, height, len = bytes.len(), "rgb frame captured");
        Frame::from_packed(bytes, width, height, PixelFormat::Rgb8)
            .ok_or_else(|| CaptureError::Decode(String::from("decoded buffer size mismatch")))
    }
}
