// Drawing arithmetic deliberately truncates floats and bridges
// signed/unsigned integers; silencing the related pedantic warnings
// keeps the rest of the crate strict.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::path::Path;

use anyhow::{Context, Result};
use dax_capture::{Frame, PixelFormat};
use dax_detect::{Detector, FaceDetection, Point2D};
use image::{ImageBuffer, ImageReader, Rgb};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_rect_mut};
use imageproc::rect::Rect;

const BOX_COLOR: Rgb<u8> = Rgb([0, 255, 0]);
const KEYPOINT_COLOR: Rgb<u8> = Rgb([255, 64, 64]);
const KEYPOINT_RADIUS: i32 = 4;
const BOX_THICKNESS: u32 = 3;

pub fn run(model: &Path, input: &Path, out: Option<&Path>) -> Result<()> {
    let mut detector =
        Detector::from_file(model).with_context(|| format!("loading {}", model.display()))?;

    let img = ImageReader::open(input)
        .with_context(|| format!("opening {}", input.display()))?
        .decode()
        .with_context(|| format!("decoding {}", input.display()))?
        .to_rgb8();

    let (width, height) = (img.width(), img.height());
    let raw = img.clone().into_raw();
    let frame = Frame::from_packed(raw, width, height, PixelFormat::Rgb8)
        .context("loaded image did not match its declared dimensions")?;

    let detections = detector.detect(&frame).context("running face detection")?;

    println!(
        "Detected {} face(s) in {width}x{height} image",
        detections.len()
    );
    for (i, det) in detections.iter().enumerate() {
        println!(
            "  #{i:>2} score={:.3} bbox=({:.0},{:.0})-({:.0},{:.0})",
            det.score, det.bbox.x1, det.bbox.y1, det.bbox.x2, det.bbox.y2
        );
    }

    if let Some(out_path) = out {
        let mut canvas = img;
        for det in &detections {
            draw_detection(&mut canvas, det);
        }
        canvas
            .save(out_path)
            .with_context(|| format!("writing annotated image to {}", out_path.display()))?;
        println!("Annotated image saved to {}", out_path.display());
    }

    Ok(())
}

fn draw_detection(canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, det: &FaceDetection) {
    let x = det.bbox.x1.round() as i32;
    let y = det.bbox.y1.round() as i32;
    let w = det.bbox.width().round().max(1.0) as u32;
    let h = det.bbox.height().round().max(1.0) as u32;

    for offset in 0..BOX_THICKNESS as i32 {
        let rect = Rect::at(x - offset, y - offset).of_size(
            w.saturating_add(2 * offset as u32),
            h.saturating_add(2 * offset as u32),
        );
        draw_hollow_rect_mut(canvas, rect, BOX_COLOR);
    }

    for point in [
        det.landmarks.left_eye,
        det.landmarks.right_eye,
        det.landmarks.nose,
        det.landmarks.left_mouth,
        det.landmarks.right_mouth,
    ] {
        draw_keypoint(canvas, point);
    }
}

fn draw_keypoint(canvas: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, point: Point2D) {
    draw_filled_circle_mut(
        canvas,
        (point.x.round() as i32, point.y.round() as i32),
        KEYPOINT_RADIUS,
        KEYPOINT_COLOR,
    );
}
