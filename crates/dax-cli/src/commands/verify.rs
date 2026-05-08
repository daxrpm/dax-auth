use std::path::Path;

use anyhow::{bail, Context, Result};
use dax_capture::Camera;
use dax_detect::{Detector, FaceDetection};
use dax_embed::{Embedder, Embedding};
use dax_liveness::{LivenessChecker, LivenessVerdict};
use dax_store::Vault;
use tracing::info;

const PASSPHRASE_ENV: &str = "DAX_VAULT_PASSPHRASE";

/// Cosine similarity above which a candidate is considered the same
/// person. Calibrated empirically against frontal snaps of the same
/// subject (typical 0.79-0.91 range).
const MATCH_THRESHOLD: f32 = 0.5;

pub fn run(
    user: &str,
    vault_path: &Path,
    device: u32,
    detector_path: &Path,
    recognizer_path: &Path,
    liveness_path: &Path,
) -> Result<()> {
    let passphrase = read_passphrase()?;

    let vault = Vault::open(vault_path, passphrase.as_bytes())
        .with_context(|| format!("opening vault {}", vault_path.display()))?;
    let templates = vault
        .templates_for(user)
        .with_context(|| format!("user `{user}` is not enrolled"))?;
    if templates.is_empty() {
        bail!("user `{user}` has no templates in the vault");
    }

    let mut detector = Detector::from_file(detector_path).context("loading detector")?;
    let mut embedder = Embedder::from_file(recognizer_path).context("loading recognizer")?;
    let mut liveness = LivenessChecker::from_file(liveness_path).context("loading liveness")?;
    let mut camera = Camera::open(device).context("opening camera")?;

    println!("Verifying `{user}` — capturing one frame…");
    let frame = camera.capture().context("capturing frame")?;
    let mut faces = detector.detect(&frame).context("detection")?;
    let face = match faces.len() {
        0 => bail!("no face detected"),
        1 => faces.remove(0),
        _ => pick_largest(faces),
    };

    let live_report = liveness
        .check(&frame, &face.bbox)
        .context("liveness check")?;
    if live_report.verdict != LivenessVerdict::Real {
        info!(
            real = live_report.real_prob,
            spoof = live_report.spoof_prob,
            "liveness rejected verification attempt"
        );
        bail!(
            "liveness check failed (real={:.4} spoof={:.4})",
            live_report.real_prob,
            live_report.spoof_prob
        );
    }

    let probe = embedder
        .embed(&frame, &face.landmarks)
        .context("embedding probe")?;
    let (best_index, best_score) = best_match(&probe, templates)?;

    let matched = best_score >= MATCH_THRESHOLD;
    println!("Detection score : {:.3}", face.score);
    println!("Liveness        : LIVE (real={:.4})", live_report.real_prob);
    println!(
        "Best match      : template #{best_index} cosine={best_score:.4} (threshold={MATCH_THRESHOLD})"
    );
    if matched {
        println!("Verdict         : ✓ MATCH");
        Ok(())
    } else {
        println!("Verdict         : ✗ NO MATCH");
        std::process::exit(2);
    }
}

fn best_match(probe: &Embedding, templates: &[dax_store::Template]) -> Result<(usize, f32)> {
    let mut best = (0usize, f32::NEG_INFINITY);
    for (i, t) in templates.iter().enumerate() {
        let stored = build_embedding(&t.embedding)?;
        let cosine = probe
            .cosine(&stored)
            .with_context(|| format!("comparing template #{i}"))?;
        if cosine > best.1 {
            best = (i, cosine);
        }
    }
    Ok(best)
}

fn build_embedding(raw: &[f32]) -> Result<Embedding> {
    Embedding::from_raw(raw.to_vec()).context("rebuilding stored embedding")
}

fn pick_largest(mut faces: Vec<FaceDetection>) -> FaceDetection {
    faces.sort_by(|a, b| {
        b.bbox
            .area()
            .partial_cmp(&a.bbox.area())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    faces.into_iter().next().unwrap()
}

fn read_passphrase() -> Result<String> {
    std::env::var(PASSPHRASE_ENV).with_context(|| {
        format!("environment variable `{PASSPHRASE_ENV}` is required to unlock the vault")
    })
}
