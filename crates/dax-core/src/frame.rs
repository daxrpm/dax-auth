use std::sync::Arc;

/// Pixel layout of a captured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Packed 8-bit RGB, three bytes per pixel.
    Rgb8,
    /// Single-channel 8-bit luminance, typical for IR sensors.
    Gray8,
}

impl PixelFormat {
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgb8 => 3,
            Self::Gray8 => 1,
        }
    }
}

/// An owned image frame captured from a camera device.
///
/// `data` is reference-counted to allow cheap fan-out to multiple
/// downstream consumers (detector, liveness, preview) without
/// copying pixel buffers.
#[derive(Debug, Clone)]
pub struct Frame {
    data: Arc<[u8]>,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
}

impl Frame {
    /// Construct a frame from a tightly-packed pixel buffer.
    ///
    /// Returns `None` if `data.len()` does not match `width * height *
    /// bytes_per_pixel`.
    #[must_use]
    pub fn from_packed(
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Option<Self> {
        let stride = width.checked_mul(format.bytes_per_pixel())?;
        let expected = stride.checked_mul(height)? as usize;
        if data.len() != expected {
            return None;
        }
        Some(Self {
            data: Arc::from(data.into_boxed_slice()),
            width,
            height,
            stride,
            format,
        })
    }

    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }
}
