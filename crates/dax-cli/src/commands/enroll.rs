use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use dax_capture::{Camera, Frame};
use dax_detect::{Detector, FaceDetection};
use dax_embed::{Embedder, Embedding};
use dax_liveness::{LivenessChecker, LivenessVerdict};
use dax_store::Vault;
use tracing::info;

const PASSPHRASE_ENV: &str = "DAX_VAULT_PASSPHRASE";
const PAUSE_BETWEEN_CAPTURES: Duration = Duration::from_millis(800);
/// Maximum number of retries before we give up: each retry consumes
/// one frame that fails the detection or liveness gate.
const MAX_RETRIES_PER_CAPTURE: usize = 5;

#[allow(clippy::too_many_arguments)]
pub fn run(
    user: &str,
    vault_path: &Path,
    captures: usize,
    device: u32,
    detector_path: &Path,
    recognizer_path: &Path,
    liveness_path: &Path,
) -> Result<()> {
    if captures == 0 {
        bail!("enrolment requires at least one capture");
    }
    let passphrase = read_passphrase()?;

    let mut detector = Detector::from_file(detector_path).context("loading detector")?;
    let mut embedder = Embedder::from_file(recognizer_path).context("loading recognizer")?;
    let mut liveness = LivenessChecker::from_file(liveness_path).context("loading liveness")?;
    let mut camera = Camera::open(device).context("opening camera")?;

    let mut vault = if vault_path.exists() {
        Vault::open(vault_path, passphrase.as_bytes())
            .with_context(|| format!("opening vault {}", vault_path.display()))?
    } else {
        Vault::new()
    };

    println!("Enrolling `{user}` — {captures} captures required. Move slightly between snapshots.");
    let mut embeddings = Vec::with_capacity(captures);
    while embeddings.len() < captures {
        let attempt = embeddings.len() + 1;
        match capture_one(&mut camera, &mut detector, &mut embedder, &mut liveness)? {
            CaptureOutcome::Accepted(embedding) => {
                println!(
                    "  [{}/{}] captured (norm-stable embedding length={})",
                    attempt,
                    captures,
                    embedding.len()
                );
                embeddings.push(embedding);
                thread::sleep(PAUSE_BETWEEN_CAPTURES);
            }
            CaptureOutcome::Rejected(reason) => {
                println!("  [{attempt}/{captures}] rejected: {reason}");
            }
        }
    }

    for embedding in embeddings {
        vault.add_template(user, embedding.as_slice().to_vec());
    }
    vault
        .save(vault_path, passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", vault_path.display()))?;
    info!(user, "enrolment complete");
    println!(
        "Enrolled `{user}` with {captures} templates → {}",
        vault_path.display()
    );
    Ok(())
}

enum CaptureOutcome {
    Accepted(Embedding),
    Rejected(String),
}

fn capture_one(
    camera: &mut Camera,
    detector: &mut Detector,
    embedder: &mut Embedder,
    liveness: &mut LivenessChecker,
) -> Result<CaptureOutcome> {
    for _ in 0..MAX_RETRIES_PER_CAPTURE {
        let frame = camera.capture().context("capturing frame")?;
        if let Some(emb) = accept_frame(&frame, detector, embedder, liveness)? {
            return Ok(CaptureOutcome::Accepted(emb));
        }
        thread::sleep(Duration::from_millis(150));
    }
    Ok(CaptureOutcome::Rejected(format!(
        "no valid frame within {MAX_RETRIES_PER_CAPTURE} attempts"
    )))
}

fn accept_frame(
    frame: &Frame,
    detector: &mut Detector,
    embedder: &mut Embedder,
    liveness: &mut LivenessChecker,
) -> Result<Option<Embedding>> {
    let mut faces = detector.detect(frame).context("detection")?;
    let face = match faces.len() {
        0 => return Ok(None),
        1 => faces.remove(0),
        _ => pick_largest(faces),
    };
    let report = liveness.check(frame, &face.bbox).context("liveness")?;
    if report.verdict != LivenessVerdict::Real {
        return Ok(None);
    }
    let embedding = embedder
        .embed(frame, &face.landmarks)
        .context("embedding")?;
    Ok(Some(embedding))
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
