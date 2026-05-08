use std::path::Path;

use anyhow::{bail, Context, Result};
use dax_capture::{Frame, PixelFormat};
use dax_detect::{Detector, FaceDetection};
use dax_embed::{estimate_alignment, warp_aligned, Embedder, Embedding, ALIGNED_SIZE};
use image::{ImageBuffer, ImageReader, Rgb};

pub fn run(
    detector_path: &Path,
    recognizer_path: &Path,
    input: &Path,
    aligned_out: Option<&Path>,
) -> Result<()> {
    let mut detector = Detector::from_file(detector_path)
        .with_context(|| format!("loading detector {}", detector_path.display()))?;
    let mut embedder = Embedder::from_file(recognizer_path)
        .with_context(|| format!("loading recognizer {}", recognizer_path.display()))?;

    let frame = read_frame(input)?;
    let face = pick_top_face(detector.detect(&frame).context("detection")?, input)?;

    if let Some(path) = aligned_out {
        let transform =
            estimate_alignment(&face.landmarks).context("estimating alignment for debug")?;
        let aligned = warp_aligned(&frame, &transform).context("warping aligned face")?;
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_raw(ALIGNED_SIZE, ALIGNED_SIZE, aligned)
                .context("aligned canvas size mismatch")?;
        img.save(path)
            .with_context(|| format!("writing aligned face to {}", path.display()))?;
        println!("Aligned face saved to {}", path.display());
    }

    let embedding = embedder
        .embed(&frame, &face.landmarks)
        .context("embedding")?;

    print_summary(&face, &embedding);
    Ok(())
}

pub fn read_frame(path: &Path) -> Result<Frame> {
    let img = ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?
        .to_rgb8();
    let (w, h) = (img.width(), img.height());
    Frame::from_packed(img.into_raw(), w, h, PixelFormat::Rgb8)
        .context("decoded image dimensions did not match buffer size")
}

pub fn pick_top_face(mut detections: Vec<FaceDetection>, source: &Path) -> Result<FaceDetection> {
    if detections.is_empty() {
        bail!("no face detected in {}", source.display());
    }
    detections.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(detections.into_iter().next().unwrap())
}

fn print_summary(face: &FaceDetection, embedding: &Embedding) {
    let preview = embedding
        .as_slice()
        .iter()
        .take(8)
        .map(|v| format!("{v:+.4}"))
        .collect::<Vec<_>>()
        .join(", ");
    let l2: f32 = embedding
        .as_slice()
        .iter()
        .map(|v| v * v)
        .sum::<f32>()
        .sqrt();

    println!("Face score : {:.3}", face.score);
    println!("Embedding  : dim={}, L2={l2:.6}", embedding.len());
    println!("First 8    : [{preview}, ...]");
}
