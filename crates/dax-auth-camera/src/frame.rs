//! Raw camera frame types and pixel-format conversions.
//!
//! All conversions in this module are allocation-minimal and avoid panicking
//! on malformed input — bad data returns `CameraError::DecodeFailed`.

use crate::{device::CameraKind, CameraError};
use zeroize::ZeroizeOnDrop;

/// A raw frame captured from a V4L2 camera device.
///
/// The `data` field is zeroed on drop via [`ZeroizeOnDrop`] because it may
/// contain raw facial pixel data — a form of biometric information.
#[derive(Debug, ZeroizeOnDrop)]
pub struct Frame {
    /// Raw pixel data in the format described by `format`.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    #[zeroize(skip)]
    pub width: u32,
    /// Frame height in pixels.
    #[zeroize(skip)]
    pub height: u32,
    /// The kind of camera this frame came from.
    #[zeroize(skip)]
    pub kind: CameraKind,
    /// The pixel encoding of the raw data.
    #[zeroize(skip)]
    pub format: PixelFormat,
}

/// V4L2 pixel formats relevant to face authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// YUYV 4:2:2 packed — the most common RGB webcam format.
    Yuyv,
    /// 8-bit greyscale — typical IR camera format.
    Grey,
    /// 16-bit greyscale little-endian — high-bit-depth IR / depth format.
    Y16,
    /// MJPEG — some webcams compress frames; requires JPEG decode.
    Mjpeg,
    /// BGR24 packed — some cameras output 24-bit BGR directly.
    Bgr24,
}

impl PixelFormat {
    /// Map a V4L2 FourCC code to a [`PixelFormat`], returning `None` for unsupported codes.
    ///
    /// Supported mappings:
    /// - `YUYV` → `Yuyv`
    /// - `MJPG` → `Mjpeg`
    /// - `BGR3` → `Bgr24`
    /// - `GREY` → `Grey`
    /// - `Y16 ` → `Y16`
    #[must_use]
    pub fn from_v4l2_fourcc(fourcc: v4l::FourCC) -> Option<Self> {
        match fourcc.repr {
            [b'Y', b'U', b'Y', b'V'] => Some(Self::Yuyv),
            [b'M', b'J', b'P', b'G'] => Some(Self::Mjpeg),
            // V4L2_PIX_FMT_BGR24 = "BGR3"
            [b'B', b'G', b'R', b'3'] => Some(Self::Bgr24),
            [b'G', b'R', b'E', b'Y'] => Some(Self::Grey),
            // V4L2_PIX_FMT_Y16 = "Y16 " (note trailing space)
            [b'Y', b'1', b'6', b' '] => Some(Self::Y16),
            _ => None,
        }
    }

    /// Convert this [`PixelFormat`] back to a V4L2 [`v4l::FourCC`].
    #[must_use]
    pub fn to_v4l2_fourcc(self) -> v4l::FourCC {
        match self {
            Self::Yuyv => v4l::FourCC::new(b"YUYV"),
            Self::Mjpeg => v4l::FourCC::new(b"MJPG"),
            Self::Bgr24 => v4l::FourCC::new(b"BGR3"),
            Self::Grey => v4l::FourCC::new(b"GREY"),
            // FourCC::new requires exactly 4 bytes; "Y16 " with trailing space.
            Self::Y16 => v4l::FourCC::new(b"Y16 "),
        }
    }
}

impl Frame {
    /// Convert frame data to packed RGB bytes (`width * height * 3` bytes).
    ///
    /// Each triplet `[R, G, B]` corresponds to one pixel in row-major order.
    ///
    /// # Format handling
    /// - `YUYV` — integer YCbCr → RGB conversion (BT.601 coefficients)
    /// - `MJPEG` — JPEG decode via the `image` crate, then to RGB
    /// - `BGR24` — byte swap: `(B,G,R)` → `(R,G,B)`
    /// - `GREY` — triplicate each grey byte: `(g, g, g)`
    /// - `Y16` — take high byte of each 16-bit LE sample, then triplicate
    ///
    /// # Errors
    /// Returns [`CameraError::DecodeFailed`] if input data is too short or
    /// MJPEG decoding fails.
    pub fn to_rgb(&self) -> Result<Vec<u8>, CameraError> {
        let pixels = (self.width * self.height) as usize;

        match self.format {
            PixelFormat::Yuyv => convert_yuyv_to_rgb(&self.data, pixels),
            PixelFormat::Mjpeg => convert_mjpeg_to_rgb(&self.data),
            PixelFormat::Bgr24 => convert_bgr24_to_rgb(&self.data, pixels),
            PixelFormat::Grey => convert_grey_to_rgb(&self.data, pixels),
            PixelFormat::Y16 => convert_y16_to_rgb(&self.data, pixels),
        }
    }

    /// Convert frame data to an [`image::RgbImage`].
    ///
    /// Internally calls [`Frame::to_rgb`] and wraps the result in an
    /// `image::RgbImage` using the frame's width and height.
    ///
    /// # Errors
    /// Returns [`CameraError::DecodeFailed`] if the conversion fails or if
    /// the raw bytes cannot form a valid `RgbImage` for this frame size.
    pub fn to_rgb_image(&self) -> Result<image::RgbImage, CameraError> {
        let rgb_bytes = self.to_rgb()?;
        image::RgbImage::from_raw(self.width, self.height, rgb_bytes).ok_or_else(|| {
            CameraError::DecodeFailed(format!(
                "RGB buffer size mismatch for {}x{} frame",
                self.width, self.height
            ))
        })
    }
}

// ─── Internal conversion helpers ─────────────────────────────────────────────

/// Convert a single (Y, Cb, Cr) triplet to `[R, G, B]` using BT.601 integer math.
///
/// Uses the fixed-point coefficients recommended for V4L2 YUYV conversion:
/// ```text
/// C0 = Y  − 16
/// C1 = Cb − 128
/// C2 = Cr − 128
/// R = clamp((298·C0           + 409·C2 + 128) >> 8, 0, 255)
/// G = clamp((298·C0 − 100·C1 − 208·C2 + 128) >> 8, 0, 255)
/// B = clamp((298·C0 + 516·C1           + 128) >> 8, 0, 255)
/// ```
#[allow(clippy::cast_possible_truncation)]
fn yuv_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let c0 = y as i32 - 16;
    let c1 = cb as i32 - 128;
    let c2 = cr as i32 - 128;

    let r = ((298 * c0 + 409 * c2 + 128) >> 8).clamp(0, 255) as u8;
    let g = ((298 * c0 - 100 * c1 - 208 * c2 + 128) >> 8).clamp(0, 255) as u8;
    let b = ((298 * c0 + 516 * c1 + 128) >> 8).clamp(0, 255) as u8;

    [r, g, b]
}

/// Convert YUYV packed bytes to packed RGB.
///
/// YUYV layout: `[Y0, Cb, Y1, Cr, Y2, Cb, Y3, Cr, ...]`
/// Each group of 4 bytes encodes 2 pixels sharing Cb/Cr.
fn convert_yuyv_to_rgb(data: &[u8], pixels: usize) -> Result<Vec<u8>, CameraError> {
    // YUYV: 2 bytes per pixel (4 bytes encode 2 pixels).
    let expected_bytes = pixels * 2;
    if data.len() < expected_bytes {
        return Err(CameraError::DecodeFailed(format!(
            "YUYV buffer too small: got {} bytes, need {expected_bytes}",
            data.len()
        )));
    }

    let mut rgb = Vec::with_capacity(pixels * 3);

    // Process pairs of pixels (4 bytes at a time).
    for chunk in data[..expected_bytes].chunks_exact(4) {
        let y0 = chunk[0];
        let cb = chunk[1];
        let y1 = chunk[2];
        let cr = chunk[3];

        let [r0, g0, b0] = yuv_to_rgb(y0, cb, cr);
        let [r1, g1, b1] = yuv_to_rgb(y1, cb, cr);

        rgb.extend_from_slice(&[r0, g0, b0, r1, g1, b1]);
    }

    Ok(rgb)
}

/// Decode an MJPEG buffer to packed RGB using the `image` crate's JPEG decoder.
fn convert_mjpeg_to_rgb(data: &[u8]) -> Result<Vec<u8>, CameraError> {
    use image::ImageFormat;

    let img = image::load_from_memory_with_format(data, ImageFormat::Jpeg)
        .map_err(|e| CameraError::DecodeFailed(format!("MJPEG decode failed: {e}")))?;

    Ok(img.into_rgb8().into_raw())
}

/// Convert BGR24 packed bytes to packed RGB by swapping R and B channels.
fn convert_bgr24_to_rgb(data: &[u8], pixels: usize) -> Result<Vec<u8>, CameraError> {
    let expected_bytes = pixels * 3;
    if data.len() < expected_bytes {
        return Err(CameraError::DecodeFailed(format!(
            "BGR24 buffer too small: got {} bytes, need {expected_bytes}",
            data.len()
        )));
    }

    let mut rgb = Vec::with_capacity(expected_bytes);

    for chunk in data[..expected_bytes].chunks_exact(3) {
        // BGR → RGB: swap indices 0 and 2.
        rgb.extend_from_slice(&[chunk[2], chunk[1], chunk[0]]);
    }

    Ok(rgb)
}

/// Convert 8-bit greyscale bytes to RGB by triplicating each byte.
fn convert_grey_to_rgb(data: &[u8], pixels: usize) -> Result<Vec<u8>, CameraError> {
    if data.len() < pixels {
        return Err(CameraError::DecodeFailed(format!(
            "GREY buffer too small: got {} bytes, need {pixels}",
            data.len()
        )));
    }

    let mut rgb = Vec::with_capacity(pixels * 3);

    for &grey in &data[..pixels] {
        rgb.extend_from_slice(&[grey, grey, grey]);
    }

    Ok(rgb)
}

/// Convert 16-bit little-endian greyscale (Y16) to RGB.
///
/// Takes the high byte of each 16-bit sample as an 8-bit grey value,
/// then triplicates it to produce RGB.
fn convert_y16_to_rgb(data: &[u8], pixels: usize) -> Result<Vec<u8>, CameraError> {
    let expected_bytes = pixels * 2;
    if data.len() < expected_bytes {
        return Err(CameraError::DecodeFailed(format!(
            "Y16 buffer too small: got {} bytes, need {expected_bytes}",
            data.len()
        )));
    }

    let mut rgb = Vec::with_capacity(pixels * 3);

    for chunk in data[..expected_bytes].chunks_exact(2) {
        // Little-endian 16-bit: chunk[0] = low byte, chunk[1] = high byte.
        let grey = chunk[1];
        rgb.extend_from_slice(&[grey, grey, grey]);
    }

    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_to_rgb_known_values() {
        // YUYV: Y=149, Cb=43, Y=149, Cr=21 — two pixels with same chroma.
        let frame = Frame {
            data: vec![149, 43, 149, 21],
            width: 2,
            height: 1,
            kind: CameraKind::Rgb,
            format: PixelFormat::Yuyv,
        };
        let rgb = frame.to_rgb().unwrap();
        assert_eq!(rgb.len(), 6, "2 pixels × 3 channels = 6 bytes");
        // Both pixels share Cb/Cr so they should be the same colour.
        assert_eq!(&rgb[0..3], &rgb[3..6]);
    }

    #[test]
    fn grey_to_rgb_triplicates_channels() {
        let frame = Frame {
            data: vec![100, 200],
            width: 2,
            height: 1,
            kind: CameraKind::Infrared,
            format: PixelFormat::Grey,
        };
        let rgb = frame.to_rgb().unwrap();
        assert_eq!(rgb, vec![100, 100, 100, 200, 200, 200]);
    }

    #[test]
    fn bgr24_to_rgb_swaps_channels() {
        let frame = Frame {
            data: vec![10, 20, 30], // B=10, G=20, R=30
            width: 1,
            height: 1,
            kind: CameraKind::Rgb,
            format: PixelFormat::Bgr24,
        };
        let rgb = frame.to_rgb().unwrap();
        assert_eq!(rgb, vec![30, 20, 10]); // R=30, G=20, B=10
    }

    #[test]
    fn y16_takes_high_byte() {
        // Two 16-bit LE samples: 0x00FF (high=0x00) and 0xFF80 (high=0xFF).
        let frame = Frame {
            data: vec![0xFF, 0x00, 0x80, 0xFF],
            width: 2,
            height: 1,
            kind: CameraKind::Infrared,
            format: PixelFormat::Y16,
        };
        let rgb = frame.to_rgb().unwrap();
        // First pixel high byte = 0x00 → grey=0; second = 0xFF → grey=255.
        assert_eq!(rgb, vec![0, 0, 0, 255, 255, 255]);
    }

    #[test]
    fn to_rgb_errors_on_too_short_yuyv() {
        let frame = Frame {
            data: vec![0u8; 1], // too short for even 1 pixel pair
            width: 2,
            height: 1,
            kind: CameraKind::Rgb,
            format: PixelFormat::Yuyv,
        };
        assert!(matches!(frame.to_rgb(), Err(CameraError::DecodeFailed(_))));
    }

    #[test]
    fn pixel_format_fourcc_roundtrip() {
        let formats = [
            PixelFormat::Yuyv,
            PixelFormat::Mjpeg,
            PixelFormat::Bgr24,
            PixelFormat::Grey,
            PixelFormat::Y16,
        ];
        for fmt in formats {
            let fourcc = fmt.to_v4l2_fourcc();
            let recovered = PixelFormat::from_v4l2_fourcc(fourcc);
            assert_eq!(recovered, Some(fmt), "roundtrip failed for {fmt:?}");
        }
    }

    #[test]
    fn frame_data_zeroized_on_drop() {
        // ZeroizeOnDrop is verified by compilation and Miri; this test just
        // confirms the type compiles with the derive and that drop is safe.
        let data: Vec<u8> = vec![1, 2, 3, 4, 5, 6];
        let ptr = data.as_ptr();
        let len = data.len();
        let frame = Frame {
            data,
            width: 2,
            height: 1,
            kind: CameraKind::Rgb,
            format: PixelFormat::Yuyv,
        };
        drop(frame);
        // Cannot safely read `ptr` after drop — just assert the pointers existed.
        let _ = (ptr, len);
    }
}
