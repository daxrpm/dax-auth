// IR cross-check builds a fake-RGB frame from an 8-bit luminance
// buffer. Both casts and the channel replication happen once per
// verify, well below any pedantic threshold.
#![allow(clippy::cast_precision_loss, clippy::similar_names)]

use std::path::Path;

use dax_capture::{Camera, Frame, IrCamera, PixelFormat};
use dax_detect::{Bbox, Detector, FaceDetection};
use dax_embed::{Embedder, Embedding};
use dax_liveness::{LivenessChecker, LivenessVerdict};
use dax_store::Vault;
use tracing::{debug, info, warn};

use crate::error::{RuntimeError, RuntimeResult};

/// Default cosine similarity threshold for accepting a verify
/// attempt. `ArcFace` was calibrated for FAR ≲ 1e-5 around cosine
/// 0.6–0.7, and frontal snaps of the same subject score 0.79–0.91
/// in our own logs, so 0.6 keeps comfortable headroom while staying
/// well above the cross-subject regime (typically < 0.3). Operators
/// who hit pose-related drops can lower it via
/// `[security] match_threshold` in `/etc/dax-auth/config.toml`.
pub const DEFAULT_MATCH_THRESHOLD: f32 = 0.6;

/// Distance tolerance between the RGB and IR face-center positions
/// after both are normalised to `[0, 1]`. The two sensors sit a few
/// centimetres apart on the laptop bezel so a small parallax is
/// expected; 0.20 (20% of the frame) is generous enough to absorb
/// it and tight enough to flag spoofs where IR sees no face or one
/// in the wrong place.
pub const DEFAULT_IR_CENTER_TOLERANCE: f32 = 0.20;

/// Static configuration for a single verification attempt.
#[derive(Debug, Clone)]
pub struct VerifyConfig<'a> {
    pub user: &'a str,
    pub vault_path: &'a Path,
    pub passphrase: &'a [u8],
    pub camera_index: u32,
    /// `Some(N)` enables Hello-style RGB↔IR cross-check against
    /// `/dev/videoN`; `None` runs RGB-only and skips the check.
    pub ir_camera_index: Option<u32>,
    pub detector_path: &'a Path,
    pub recognizer_path: &'a Path,
    pub liveness_path: &'a Path,
    pub match_threshold: f32,
    pub ir_center_tolerance: f32,
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
            ir_camera_index: None,
            detector_path,
            recognizer_path,
            liveness_path,
            match_threshold: DEFAULT_MATCH_THRESHOLD,
            ir_center_tolerance: DEFAULT_IR_CENTER_TOLERANCE,
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
    /// Result of the optional RGB↔IR cross-check.
    pub ir_check: IrCheckOutcome,
    pub reason: VerifyReason,
}

/// Status of the IR cross-check for the verify attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrCheckOutcome {
    /// The host has no IR sensor configured (or the caller chose to
    /// disable the check).
    Disabled,
    /// IR detection found a face whose centre matched the RGB face.
    Matched,
    /// IR detection found no face at all — typical of a phone
    /// screen, photo or display being held in front of the camera.
    NoFace,
    /// IR detection found a face but it was off-position relative to
    /// the RGB detection — typical of a mask held next to the user
    /// or two attackers.
    OffPosition,
}

/// Why a verification attempt resolved as it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyReason {
    Match,
    BelowThreshold,
    LivenessSpoof,
    /// The IR sensor disagreed with the RGB sensor about whether a
    /// face was present. Treated as a spoof.
    IrCrossCheckFailed,
}

/// Run the full verification pipeline.
///
/// 1. Open the vault and look up the user's templates.
/// 2. Capture a single RGB frame.
/// 3. Detect a face (largest if multiple).
/// 4. (Optional) Capture an IR frame and verify a face is present
///    in the same approximate position. Reject if the two sensors
///    disagree.
/// 5. Reject the attempt if liveness flags it as a spoof.
/// 6. Compute the embedding and find the highest cosine across the
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
    debug!(score = face.score, "rgb face accepted");

    let ir_check = run_ir_cross_check(config, &face.bbox, &frame, &mut detector)?;
    if matches!(
        ir_check,
        IrCheckOutcome::NoFace | IrCheckOutcome::OffPosition
    ) {
        info!(?ir_check, "ir cross-check rejected verify attempt");
        return Ok(VerifyOutcome {
            matched: false,
            face_score: face.score,
            liveness_real: 0.0,
            liveness_spoof: 0.0,
            best_template: 0,
            best_cosine: 0.0,
            ir_check,
            reason: VerifyReason::IrCrossCheckFailed,
        });
    }

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
            ir_check,
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
        ir_check,
        reason: if matched {
            VerifyReason::Match
        } else {
            VerifyReason::BelowThreshold
        },
    })
}

/// Capture an IR frame and verify a face is present in the same
/// approximate position as the RGB one. Returns `Disabled` when the
/// host has no IR sensor configured.
fn run_ir_cross_check(
    config: &VerifyConfig<'_>,
    rgb_bbox: &Bbox,
    rgb_frame: &Frame,
    detector: &mut Detector,
) -> RuntimeResult<IrCheckOutcome> {
    let Some(ir_index) = config.ir_camera_index else {
        return Ok(IrCheckOutcome::Disabled);
    };

    let mut ir_camera = match IrCamera::open(ir_index) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "ir camera unavailable; cross-check skipped");
            return Ok(IrCheckOutcome::Disabled);
        }
    };
    let ir_frame_gray = ir_camera.capture()?;
    let ir_frame_rgb = expand_gray_to_rgb(&ir_frame_gray)?;
    let ir_faces = detector.detect(&ir_frame_rgb)?;
    debug!(ir_faces = ir_faces.len(), "ir detection complete");

    if ir_faces.is_empty() {
        return Ok(IrCheckOutcome::NoFace);
    }

    if bboxes_overlap(
        rgb_bbox,
        rgb_frame,
        &ir_faces,
        &ir_frame_rgb,
        config.ir_center_tolerance,
    ) {
        Ok(IrCheckOutcome::Matched)
    } else {
        Ok(IrCheckOutcome::OffPosition)
    }
}

/// Replicate an 8-bit luminance frame across three channels so the
/// existing SCRFD wrapper (trained on RGB) can chew on it.
fn expand_gray_to_rgb(frame: &Frame) -> RuntimeResult<Frame> {
    if frame.format() != PixelFormat::Gray8 {
        return Err(RuntimeError::Config(format!(
            "expected Gray8 frame, got {:?}",
            frame.format()
        )));
    }
    let width = frame.width();
    let height = frame.height();
    let src = frame.data();
    let mut rgb = Vec::with_capacity(src.len() * 3);
    for &g in src {
        rgb.extend_from_slice(&[g, g, g]);
    }
    Frame::from_packed(rgb, width, height, PixelFormat::Rgb8).ok_or_else(|| {
        RuntimeError::Config(String::from("ir frame buffer mismatched declared size"))
    })
}

fn bboxes_overlap(
    rgb_bbox: &Bbox,
    rgb_frame: &Frame,
    ir_faces: &[FaceDetection],
    ir_frame: &Frame,
    tolerance: f32,
) -> bool {
    let rgb_w = rgb_frame.width() as f32;
    let rgb_h = rgb_frame.height() as f32;
    let ir_w = ir_frame.width() as f32;
    let ir_h = ir_frame.height() as f32;
    if rgb_w <= 0.0 || rgb_h <= 0.0 || ir_w <= 0.0 || ir_h <= 0.0 {
        return false;
    }
    let rgb_cx = (rgb_bbox.x1 + rgb_bbox.x2) * 0.5 / rgb_w;
    let rgb_cy = (rgb_bbox.y1 + rgb_bbox.y2) * 0.5 / rgb_h;

    ir_faces.iter().any(|f| {
        let ir_cx = (f.bbox.x1 + f.bbox.x2) * 0.5 / ir_w;
        let ir_cy = (f.bbox.y1 + f.bbox.y2) * 0.5 / ir_h;
        let dx = ir_cx - rgb_cx;
        let dy = ir_cy - rgb_cy;
        (dx * dx + dy * dy).sqrt() < tolerance
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
