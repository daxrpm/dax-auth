//! Direct V4L2 path for infrared cameras.
//!
//! We bypass `nokhwa` here because the 0.10 release cannot negotiate
//! the V4L2 `GREY` `FourCC` that Windows-Hello-class IR sensors use.
//! `v4l-rs` gives us a tight, predictable wrapper over the kernel
//! interface and matches the convention used by `Howdy`.

use dax_core::{Frame, PixelFormat};
use tracing::debug;
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::FourCC;
use v4l::{Device, Format};

use crate::error::{CaptureError, CaptureResult};

/// Default capture mode for the IR sensor on the reference hardware.
const DEFAULT_WIDTH: u32 = 640;
const DEFAULT_HEIGHT: u32 = 360;
/// V4L2 8-bit grayscale `FourCC`.
const GREY_FOURCC: &[u8; 4] = b"GREY";
/// Number of MMAP buffers to allocate for the streaming queue.
const MMAP_BUFFER_COUNT: u32 = 4;

/// V4L2 handle for an 8-bit grayscale (IR) camera.
///
/// The MMAP streaming queue is created per-capture to avoid the
/// self-referential lifetime that would arise from caching it on the
/// struct. For single-shot face authentication this overhead is
/// negligible (~2ms) and trades cleanly against the unsafe code that
/// caching would require.
pub struct IrCamera {
    device: Device,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for IrCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrCamera")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Number of frames to discard so the sensor can settle before we
/// keep one. The first one or two MJPEG/GREY buffers after starting
/// a V4L2 stream are commonly blank.
const WARMUP_FRAMES: usize = 1;

impl IrCamera {
    pub fn open(index: u32) -> CaptureResult<Self> {
        let device = Device::new(index as usize)
            .map_err(|e| CaptureError::DeviceOpen(format!("/dev/video{index}: {e}")))?;

        let mut format = device
            .format()
            .map_err(|e| CaptureError::DeviceOpen(format!("query format: {e}")))?;
        format.width = DEFAULT_WIDTH;
        format.height = DEFAULT_HEIGHT;
        format.fourcc = FourCC::new(GREY_FOURCC);

        let negotiated: Format = device
            .set_format(&format)
            .map_err(|e| CaptureError::DeviceOpen(format!("set GREY format: {e}")))?;

        if negotiated.fourcc != FourCC::new(GREY_FOURCC) {
            return Err(CaptureError::DeviceOpen(format!(
                "device {index} did not accept GREY format (got {:?})",
                negotiated.fourcc
            )));
        }

        debug!(
            index,
            width = negotiated.width,
            height = negotiated.height,
            "ir camera opened"
        );

        Ok(Self {
            device,
            width: negotiated.width,
            height: negotiated.height,
        })
    }

    /// Capture a single grayscale frame.
    pub fn capture(&mut self) -> CaptureResult<Frame> {
        let width = self.width;
        let height = self.height;
        let expected = (width * height) as usize;

        let mut stream = Stream::with_buffers(&self.device, Type::VideoCapture, MMAP_BUFFER_COUNT)
            .map_err(|e| CaptureError::Stream(format!("init mmap stream: {e}")))?;

        for _ in 0..WARMUP_FRAMES {
            stream
                .next()
                .map_err(|e| CaptureError::Stream(format!("warmup: {e}")))?;
        }

        let (raw, _meta) = stream
            .next()
            .map_err(|e| CaptureError::Stream(e.to_string()))?;
        if raw.len() < expected {
            return Err(CaptureError::Decode(format!(
                "ir frame too small: {} bytes, expected {expected}",
                raw.len()
            )));
        }
        let bytes = raw[..expected].to_vec();
        debug!(width, height, len = bytes.len(), "ir frame captured");

        Frame::from_packed(bytes, width, height, PixelFormat::Gray8)
            .ok_or_else(|| CaptureError::Decode(String::from("ir frame size mismatch")))
    }
}
