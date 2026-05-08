// Image-processing arithmetic deliberately uses lossy numeric casts
// (u32 ↔ f32, f32 → u32 after rounding); silence the related pedantic
// warnings here. `similar_names` allows the natural src/dst naming.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

use dax_capture::{Frame, PixelFormat};
use nalgebra::Vector2;

use crate::align::{AffineTransform, ALIGNED_SIZE};
use crate::error::{EmbedError, EmbedResult};

/// Warp the source RGB frame onto a 112×112 packed-RGB canvas using
/// the inverse of `transform` and bilinear interpolation.
pub fn warp_aligned(frame: &Frame, transform: &AffineTransform) -> EmbedResult<Vec<u8>> {
    if frame.format() != PixelFormat::Rgb8 {
        return Err(EmbedError::Warp(format!(
            "expected RGB8 frame, got {:?}",
            frame.format()
        )));
    }

    let inverse = transform
        .invert()
        .ok_or_else(|| EmbedError::Warp(String::from("alignment transform is singular")))?;

    let size = ALIGNED_SIZE as usize;
    let mut canvas = vec![0u8; size * size * 3];
    let frame_w_max = (frame.width() as f32) - 1.0;
    let frame_h_max = (frame.height() as f32) - 1.0;

    for dy in 0..size {
        for dx in 0..size {
            let dst = Vector2::new(dx as f32, dy as f32);
            let src = inverse.a * dst + inverse.t;

            if src.x < 0.0 || src.y < 0.0 || src.x >= frame_w_max || src.y >= frame_h_max {
                continue;
            }
            let pixel = bilinear_sample(frame, src.x, src.y);
            let idx = (dy * size + dx) * 3;
            canvas[idx] = pixel[0];
            canvas[idx + 1] = pixel[1];
            canvas[idx + 2] = pixel[2];
        }
    }

    Ok(canvas)
}

fn bilinear_sample(frame: &Frame, x: f32, y: f32) -> [u8; 3] {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;

    let p00 = pixel_at(frame, x0, y0);
    let p10 = pixel_at(frame, x1, y0);
    let p01 = pixel_at(frame, x0, y1);
    let p11 = pixel_at(frame, x1, y1);

    let mut out = [0u8; 3];
    for c in 0..3 {
        let v = f32::from(p00[c]) * (1.0 - dx) * (1.0 - dy)
            + f32::from(p10[c]) * dx * (1.0 - dy)
            + f32::from(p01[c]) * (1.0 - dx) * dy
            + f32::from(p11[c]) * dx * dy;
        out[c] = v.clamp(0.0, 255.0) as u8;
    }
    out
}

fn pixel_at(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
    let stride = frame.stride() as usize;
    let idx = (y as usize) * stride + (x as usize) * 3;
    let data = frame.data();
    [data[idx], data[idx + 1], data[idx + 2]]
}
