// Image-processing arithmetic deliberately uses lossy numeric casts
// (u32 ↔ f32, f32 → u32 after rounding); silencing the related
// pedantic warnings here keeps the rest of the workspace strict.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use dax_capture::{Frame, PixelFormat};
use image::{imageops::FilterType, ImageBuffer, Rgb};
use ndarray::Array4;

use crate::error::{DetectError, DetectResult};

/// Side length (square) of the SCRFD network input.
pub const INPUT_SIZE: u32 = 640;

/// Per-channel mean used by the `InsightFace` SCRFD preprocessing.
const MEAN: f32 = 127.5;
/// Per-channel scale used by the `InsightFace` SCRFD preprocessing.
const STD: f32 = 128.0;

/// Geometry needed to map model-space coordinates back to the
/// original frame.
#[derive(Debug, Clone, Copy)]
pub struct LetterboxGeometry {
    pub scale: f32,
    pub pad_x: u32,
    pub pad_y: u32,
}

/// Resize a frame into a 640x640 letterboxed RGB tensor in NCHW
/// layout, normalised the way SCRFD expects it.
pub fn preprocess(frame: &Frame) -> DetectResult<(Array4<f32>, LetterboxGeometry)> {
    if frame.format() != PixelFormat::Rgb8 {
        return Err(DetectError::Preprocess(format!(
            "expected RGB8 frame, got {:?}",
            frame.format()
        )));
    }

    let src: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(frame.width(), frame.height(), frame.data().to_vec()).ok_or_else(
            || DetectError::Preprocess(String::from("frame buffer mismatched declared size")),
        )?;

    let target = INPUT_SIZE as f32;
    let scale = (target / frame.width() as f32).min(target / frame.height() as f32);

    let new_w = (frame.width() as f32 * scale).round() as u32;
    let new_h = (frame.height() as f32 * scale).round() as u32;

    let resized = image::imageops::resize(&src, new_w, new_h, FilterType::Triangle);

    let mut tensor = Array4::<f32>::zeros((1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize));

    for y in 0..new_h {
        for x in 0..new_w {
            let pixel = resized.get_pixel(x, y);
            tensor[[0, 0, y as usize, x as usize]] = (f32::from(pixel.0[0]) - MEAN) / STD;
            tensor[[0, 1, y as usize, x as usize]] = (f32::from(pixel.0[1]) - MEAN) / STD;
            tensor[[0, 2, y as usize, x as usize]] = (f32::from(pixel.0[2]) - MEAN) / STD;
        }
    }

    Ok((
        tensor,
        LetterboxGeometry {
            scale,
            pad_x: 0,
            pad_y: 0,
        },
    ))
}
