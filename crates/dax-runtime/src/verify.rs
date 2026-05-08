use std::path::Path;

use dax_capture::Camera;
use dax_detect::{Detector, FaceDetection};
use dax_embed::{Embedder, Embedding};
use dax_liveness::{LivenessChecker, LivenessVerdict};
use dax_store::Vault;
use tracing::{debug, info};

use crate::error::{RuntimeError, RuntimeResult};

/// Default cosine similarity threshold for accepting a verify
/// attempt. Empirically, frontal snaps of the same subject score
/// 0.79–0.91, so 0.5 leaves margin for pose variation while
/// remaining clearly distinguishable from cross-subject pairs.
pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.5;

/// Static configuration for a single verification attempt.
#[derive(Debug, Clone)]
pub struct VerifyConfig<'a> {
    pub user: &'a str,
    pub vault_path: &'a Path,
    pub passphrase: &'a [u8],
    pub camera_index: u32,
    pub detector_path: &'a Path,
    pub recognizer_path: &'a Path,
    pub liveness_path: &'a Path,
    pub match_threshold: f32,
}

impl<'a> VerifyConfig<'a> {
    #[must_use]
    pub fn new(
        user: &'a str,
        vault_path: &'a Path,
        passphrase: &'a [u8],
        detector_path: &'a Path,
        recognizer_path: &'a Path,
        liveness_path: &'a Path,
    ) -> Self {
        Self {
            user,
            vault_path,
            passphrase,
            camera_index: 0,
            detector_path,
            recognizer_path,
            liveness_path,
            match_threshold: DEFAULT_MATCH_THRESHOLD,
        }
    }
}

/// Outcome of [`verify_face`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerifyOutcome {
    pub matched: bool,
    pub face_score: f32,
    pub liveness_real: f32,
    pub liveness_spoof: f32,
    pub best_template: usize,
    pub best_cosine: f32,
    pub reason: VerifyReason,
}

/// Why a verification attempt resolved as it did. Useful for
/// pam-side logging where we cannot show the full report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyReason {
    Match,
    BelowThreshold,
    LivenessSpoof,
}

/// Run the full verification pipeline.
///
/// 1. Open the vault and look up the user's templates.
/// 2. Capture a single frame.
/// 3. Detect a face (largest if multiple).
/// 4. Reject the attempt if liveness flags it as a spoof.
/// 5. Compute the embedding and find the highest cosine across the
///    stored templates.
pub fn verify_face(config: &VerifyConfig<'_>) -> RuntimeResult<VerifyOutcome> {
    let vault = Vault::open(config.vault_path, config.passphrase)?;
    let templates = vault
        .templates_for(config.user)
        .ok_or_else(|| RuntimeError::UserNotEnrolled(config.user.to_string()))?;
    if templates.is_empty() {
        return Err(RuntimeError::EmptyTemplates(config.user.to_string()));
    }

    let mut detector = Detector::from_file(config.detector_path)?;
    let mut embedder = Embedder::from_file(config.recognizer_path)?;
    let mut liveness = LivenessChecker::from_file(config.liveness_path)?;
    let mut camera = Camera::open(config.camera_index)?;

    let frame = camera.capture()?;
    let faces = detector.detect(&frame)?;
    let face = pick_largest(faces).ok_or(RuntimeError::NoFace)?;
    debug!(score = face.score, "face accepted");

    let live = liveness.check(&frame, &face.bbox)?;
    if live.verdict != LivenessVerdict::Real {
        info!(
            real = live.real_prob,
            spoof = live.spoof_prob,
            "liveness rejected verify attempt"
        );
        return Ok(VerifyOutcome {
            matched: false,
            face_score: face.score,
            liveness_real: live.real_prob,
            liveness_spoof: live.spoof_prob,
            best_template: 0,
            best_cosine: 0.0,
            reason: VerifyReason::LivenessSpoof,
        });
    }

    let probe = embedder.embed(&frame, &face.landmarks)?;
    let (best_template, best_cosine) = best_match(&probe, templates)?;
    let matched = best_cosine >= config.match_threshold;

    Ok(VerifyOutcome {
        matched,
        face_score: face.score,
        liveness_real: live.real_prob,
        liveness_spoof: live.spoof_prob,
        best_template,
        best_cosine,
        reason: if matched {
            VerifyReason::Match
        } else {
            VerifyReason::BelowThreshold
        },
    })
}

fn pick_largest(mut faces: Vec<FaceDetection>) -> Option<FaceDetection> {
    if faces.is_empty() {
        return None;
    }
    faces.sort_by(|a, b| {
        b.bbox
            .area()
            .partial_cmp(&a.bbox.area())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    faces.into_iter().next()
}

fn best_match(probe: &Embedding, templates: &[dax_store::Template]) -> RuntimeResult<(usize, f32)> {
    let mut best = (0usize, f32::NEG_INFINITY);
    for (i, t) in templates.iter().enumerate() {
        let stored = Embedding::from_raw(t.embedding.clone())?;
        let cosine = probe.cosine(&stored)?;
        if cosine > best.1 {
            best = (i, cosine);
        }
    }
    Ok(best)
}
