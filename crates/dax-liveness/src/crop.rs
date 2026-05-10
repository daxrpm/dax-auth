// Crop arithmetic mixes f32 ↔ i32 ↔ u32 deliberately; mirrored
// from the reference Python implementation. `similar_names` allows
// the natural max_{w,h}_scale / new_{w,h} pairs.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

use dax_capture::{Frame, PixelFormat};
use dax_detect::Bbox;
use image::{imageops::FilterType, ImageBuffer, Rgb};

use crate::error::{LivenessError, LivenessResult};

/// Resize an RGB face crop to `target_size` × `target_size` BGR
/// pixels (HWC), matching the `MiniFASNet` Python reference. The
/// crop region is centred on the bounding box and expanded by
/// `scale`, then clamped to the source image bounds.
pub fn crop_face_to_bgr(
    frame: &Frame,
    bbox: &Bbox,
    scale: f32,
    target_size: u32,
) -> LivenessResult<Vec<u8>> {
    if frame.format() != PixelFormat::Rgb8 {
        return Err(LivenessError::Preprocess(format!(
            "expected RGB8 frame, got {:?}",
            frame.format()
        )));
    }

    let src_w = frame.width();
    let src_h = frame.height();
    if src_w == 0 || src_h == 0 {
        return Err(LivenessError::Preprocess(String::from(
            "frame has zero dimensions",
        )));
    }

    let box_w = (bbox.x2 - bbox.x1).max(1.0);
    let box_h = (bbox.y2 - bbox.y1).max(1.0);

    let max_w_scale = ((src_w - 1) as f32) / box_w;
    let max_h_scale = ((src_h - 1) as f32) / box_h;
    let effective_scale = scale.min(max_w_scale).min(max_h_scale);

    let new_w = box_w * effective_scale;
    let new_h = box_h * effective_scale;
    let center_x = bbox.x1 + box_w * 0.5;
    let center_y = bbox.y1 + box_h * 0.5;

    let x1 = (center_x - new_w * 0.5).max(0.0) as u32;
    let y1 = (center_y - new_h * 0.5).max(0.0) as u32;
    let x2 = ((center_x + new_w * 0.5) as u32).min(src_w - 1);
    let y2 = ((center_y + new_h * 0.5) as u32).min(src_h - 1);

    if x2 <= x1 || y2 <= y1 {
        return Err(LivenessError::Preprocess(format!(
            "degenerate crop region: ({x1},{y1})-({x2},{y2})"
        )));
    }

    let crop_w = x2 - x1 + 1;
    let crop_h = y2 - y1 + 1;
    let mut crop_rgb = vec![0u8; (crop_w * crop_h * 3) as usize];
    let stride = frame.stride() as usize;
    let data = frame.data();
    for row in 0..crop_h {
        let src_y = (y1 + row) as usize;
        let src_off = src_y * stride + (x1 as usize) * 3;
        let dst_off = (row * crop_w * 3) as usize;
        let len = (crop_w * 3) as usize;
        crop_rgb[dst_off..dst_off + len].copy_from_slice(&data[src_off..src_off + len]);
    }

    let crop_buf: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(crop_w, crop_h, crop_rgb)
            .ok_or_else(|| LivenessError::Preprocess(String::from("crop buffer size mismatch")))?;
    let resized =
        image::imageops::resize(&crop_buf, target_size, target_size, FilterType::Triangle);

    // Convert RGB → BGR in place so the caller can feed it directly
    // into the network expected by MiniFASNet.
    let mut bgr = resized.into_raw();
    debug_assert_eq!(bgr.len(), (target_size * target_size * 3) as usize);
    for px in bgr.chunks_exact_mut(3) {
        px.swap(0, 2);
    }
    Ok(bgr)
}
