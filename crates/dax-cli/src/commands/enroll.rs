use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use dax_capture::{Camera, Frame};
use dax_detect::{Detector, FaceDetection};
use dax_embed::{Embedder, Embedding};
use dax_liveness::{LivenessChecker, LivenessVerdict};
use dax_store::Vault;
use tracing::info;

use crate::resolve::{default_user, resolve, Overrides};

const PAUSE_BETWEEN_CAPTURES: Duration = Duration::from_millis(800);
const MAX_RETRIES_PER_CAPTURE: usize = 5;

#[derive(Debug)]
pub struct Args {
    pub user: Option<String>,
    pub vault: Option<PathBuf>,
    pub captures: usize,
    pub device: Option<u32>,
    pub detector: Option<PathBuf>,
    pub recognizer: Option<PathBuf>,
    pub liveness_model: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    if args.captures == 0 {
        bail!("enrolment requires at least one capture");
    }

    let user = match args.user {
        Some(u) => u,
        None => default_user().context("--user not provided and could not be inferred")?,
    };

    let cfg = resolve(Overrides {
        vault: args.vault.as_deref(),
        detector: args.detector.as_deref(),
        recognizer: args.recognizer.as_deref(),
        liveness: args.liveness_model.as_deref(),
        camera_index: args.device,
    })?;

    let mut detector = Detector::from_file(&cfg.detector).context("loading detector")?;
    let mut embedder = Embedder::from_file(&cfg.recognizer).context("loading recognizer")?;
    let mut liveness = LivenessChecker::from_file(&cfg.liveness).context("loading liveness")?;
    let mut camera = Camera::open(cfg.camera_index).context("opening camera")?;

    let mut vault = if cfg.vault.exists() {
        Vault::open(&cfg.vault, cfg.passphrase.as_bytes())
            .with_context(|| format!("opening vault {}", cfg.vault.display()))?
    } else {
        Vault::new()
    };

    println!(
        "Enrolling `{user}` — {} captures required. Move slightly between snapshots.",
        args.captures
    );
    let mut embeddings = Vec::with_capacity(args.captures);
    while embeddings.len() < args.captures {
        let attempt = embeddings.len() + 1;
        match capture_one(&mut camera, &mut detector, &mut embedder, &mut liveness)? {
            CaptureOutcome::Accepted(embedding) => {
                println!(
                    "  [{attempt}/{}] captured (embedding length={})",
                    args.captures,
                    embedding.len()
                );
                embeddings.push(embedding);
                thread::sleep(PAUSE_BETWEEN_CAPTURES);
            }
            CaptureOutcome::Rejected(reason) => {
                println!("  [{attempt}/{}] rejected: {reason}", args.captures);
            }
        }
    }

    for embedding in embeddings {
        vault.add_template(&user, embedding.as_slice().to_vec());
    }
    vault
        .save(&cfg.vault, cfg.passphrase.as_bytes())
        .with_context(|| format!("saving vault {}", cfg.vault.display()))?;
    info!(user, "enrolment complete");
    println!(
        "Enrolled `{user}` with {} templates → {}",
        args.captures,
        cfg.vault.display()
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
