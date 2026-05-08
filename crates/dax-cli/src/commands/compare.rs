use std::path::Path;

use anyhow::{Context, Result};
use dax_detect::Detector;
use dax_embed::{Embedder, Embedding};

use crate::commands::embed::{pick_top_face, read_frame};

const SAME_PERSON_THRESHOLD: f32 = 0.4;
const STRONG_MATCH_THRESHOLD: f32 = 0.6;

pub fn run(
    detector_path: &Path,
    recognizer_path: &Path,
    a_path: &Path,
    b_path: &Path,
) -> Result<()> {
    let mut detector = Detector::from_file(detector_path)
        .with_context(|| format!("loading detector {}", detector_path.display()))?;
    let mut embedder = Embedder::from_file(recognizer_path)
        .with_context(|| format!("loading recognizer {}", recognizer_path.display()))?;

    let a_emb = embed_image(&mut detector, &mut embedder, a_path)?;
    let b_emb = embed_image(&mut detector, &mut embedder, b_path)?;

    let cosine = a_emb.cosine(&b_emb).context("comparing embeddings")?;
    let verdict = if cosine >= STRONG_MATCH_THRESHOLD {
        "MATCH (strong)"
    } else if cosine >= SAME_PERSON_THRESHOLD {
        "MATCH (likely)"
    } else {
        "NO MATCH"
    };

    println!("Cosine similarity : {cosine:.4}");
    println!("Verdict           : {verdict}");
    println!("Thresholds        : weak={SAME_PERSON_THRESHOLD}, strong={STRONG_MATCH_THRESHOLD}");
    Ok(())
}

fn embed_image(detector: &mut Detector, embedder: &mut Embedder, path: &Path) -> Result<Embedding> {
    let frame = read_frame(path)?;
    let face = pick_top_face(detector.detect(&frame).context("detection")?, path)?;
    embedder
        .embed(&frame, &face.landmarks)
        .with_context(|| format!("embedding {}", path.display()))
}
