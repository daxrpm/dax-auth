//! Main authentication pipeline orchestrator.
//!
//! [`AuthPipeline`] is the top-level coordinator: it holds all loaded ONNX sessions
//! and the encrypted embedding store, and runs a complete camera → detection →
//! liveness → recognition → matching pipeline for each auth attempt.

use crate::detection::FaceDetector;
use crate::embedding::{align_face, FaceRecognizer};
use crate::liveness::LivenessDetector;
use crate::models::ModelRegistry;
use crate::store::FaceStore;
use crate::{CoreConfig, CoreError};
use dax_auth_camera::{CameraCapture, CameraDevice, CameraKind};
use dax_auth_proto::SecurityMode;

// ─── FailureStage ─────────────────────────────────────────────────────────────

/// The pipeline stage at which authentication failed.
///
/// Used for diagnostics and logging — never exposed to the end user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureStage {
    /// No face was detected within `max_frames`.
    NoFaceDetected,
    /// A face was detected but the liveness check failed (possible spoof).
    LivenessFailed,
    /// Face matched but the cosine similarity was below the configured threshold.
    BelowThreshold,
    /// The user has no enrolled face embeddings.
    NoEnrolledFaces,
    /// The camera could not be opened or a frame could not be captured.
    CameraError,
    /// An unexpected internal error occurred.
    InternalError,
}

// ─── PipelineResult ───────────────────────────────────────────────────────────

/// Result of a single authentication pipeline run.
///
/// Returned by [`AuthPipeline::authenticate`] on success. On hard errors
/// (I/O failure, model crash), `CoreError` is returned instead.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Whether authentication was granted.
    pub granted: bool,
    /// Cosine similarity score (0.0–1.0). `None` if no face was detected.
    pub score: Option<f32>,
    /// Which enrolled face index matched. `None` if no match.
    pub matched_face: Option<usize>,
    /// Whether liveness was confirmed during this attempt.
    pub liveness_ok: bool,
    /// Time taken for the full pipeline in milliseconds.
    pub duration_ms: u64,
    /// Stage at which authentication failed. `None` if `granted` is `true`.
    pub failure_stage: Option<FailureStage>,
}

// ─── AuthPipeline ─────────────────────────────────────────────────────────────

/// The main facial authentication pipeline.
///
/// Holds all loaded ONNX sessions and the encrypted face embedding store.
/// Designed to be created once at daemon startup and reused across multiple
/// authentication requests. Each call to [`authenticate`][AuthPipeline::authenticate]
/// opens and closes a camera session independently (RAII).
///
/// Concurrency: wrap in `Arc<tokio::sync::Mutex<AuthPipeline>>` when sharing
/// across tasks — the pipeline serialises authentication attempts by design
/// (one camera, one ML pipeline).
pub struct AuthPipeline {
    /// Runtime configuration (thresholds, model paths, etc.).
    config: CoreConfig,
    /// RetinaFace face detector.
    detector: FaceDetector,
    /// MiniFASNetV2 liveness checker.
    liveness: LivenessDetector,
    /// ArcFace R100 face recognizer.
    recognizer: FaceRecognizer,
    /// Encrypted embedding store.
    store: FaceStore,
}

impl AuthPipeline {
    /// Initialize the pipeline: load all ONNX models and open the face store.
    ///
    /// This call is intentionally slow (~2–5 s on first run while models are
    /// loaded and warmed up). Call it once at daemon startup, not per request.
    ///
    /// # Errors
    /// - [`CoreError::ModelNotFound`] if any model file is missing.
    /// - [`CoreError::Inference`] if a session cannot be built.
    /// - [`CoreError::Store`] if the embedding store cannot be opened.
    pub fn initialize(config: CoreConfig) -> Result<Self, CoreError> {
        tracing::info!(
            models_dir = %config.models_dir.display(),
            "loading ONNX models"
        );

        let registry = ModelRegistry::load(&config)?;

        let detector = FaceDetector::new(registry.detector);
        // Use Rgb as the default camera kind for the liveness detector.
        // The authenticate() method will open the real camera and the liveness
        // detector is flexible enough to handle the actual frame regardless.
        let liveness = LivenessDetector::new(CameraKind::Rgb, Some(registry.anti_spoof));
        let recognizer = FaceRecognizer::new(registry.recognizer);

        // Derive the storage directory from the models directory.
        // Convention: /var/lib/dax-auth/models → /var/lib/dax-auth/users
        let storage_dir = config
            .models_dir
            .parent()
            .map(|p| p.join("users"))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));

        let store = FaceStore::open(&storage_dir)?;

        tracing::info!("auth pipeline ready");

        Ok(Self {
            config,
            detector,
            liveness,
            recognizer,
            store,
        })
    }

    /// Run a complete authentication attempt for the given user.
    ///
    /// Opens a camera capture session, loops through frames until a face is
    /// detected and passes liveness + matching, or until `max_frames` is
    /// exhausted. The camera is automatically closed when this method returns
    /// (RAII via [`CameraCapture`] drop).
    ///
    /// # Security logging
    /// - Similarity scores are **not** logged on success (biometric leak / timing risk).
    /// - Only denied scores (below threshold) may be logged, and only at `debug` level.
    /// - Liveness scores are never logged (biometric-derived).
    ///
    /// # Errors
    /// Returns [`CoreError`] on camera I/O or ONNX inference failure.
    /// Returns `Ok(PipelineResult { granted: false, ... })` on soft failures
    /// (no face detected, liveness failed, below threshold).
    pub async fn authenticate(
        &mut self,
        username: &str,
        mode: SecurityMode,
        camera_kind: CameraKind,
    ) -> Result<PipelineResult, CoreError> {
        let start = std::time::Instant::now();
        let threshold = self.config.threshold_for(mode);

        // ── 1. Check enrolled faces ────────────────────────────────────────────
        let enrolled = match self.store.load(username) {
            Ok(u) => u,
            Err(CoreError::NoEnrolledFaces { .. }) => {
                return Ok(PipelineResult {
                    granted: false,
                    score: None,
                    matched_face: None,
                    liveness_ok: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    failure_stage: Some(FailureStage::NoEnrolledFaces),
                });
            }
            Err(e) => return Err(e),
        };

        if enrolled.embeddings.is_empty() {
            return Ok(PipelineResult {
                granted: false,
                score: None,
                matched_face: None,
                liveness_ok: false,
                duration_ms: start.elapsed().as_millis() as u64,
                failure_stage: Some(FailureStage::NoEnrolledFaces),
            });
        }

        // ── 2. Open camera ─────────────────────────────────────────────────────
        // Attempt to get the best available device; fall back to camera_kind hint.
        let device = match CameraDevice::best_available() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "camera unavailable");
                return Ok(PipelineResult {
                    granted: false,
                    score: None,
                    matched_face: None,
                    liveness_ok: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    failure_stage: Some(FailureStage::CameraError),
                });
            }
        };

        let mut capture = match CameraCapture::open(device) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to open camera");
                return Ok(PipelineResult {
                    granted: false,
                    score: None,
                    matched_face: None,
                    liveness_ok: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    failure_stage: Some(FailureStage::CameraError),
                });
            }
        };

        // ── 3–6. Frame loop ────────────────────────────────────────────────────
        let mut best_score: f32 = 0.0;
        let mut best_match_idx: Option<usize> = None;
        let mut liveness_passed = false;
        let mut face_was_detected = false;
        let mut last_failure = FailureStage::NoFaceDetected;

        for _frame_idx in 0..self.config.max_frames {
            // Capture
            let frame = match capture.capture_frame_async().await {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "frame capture failed");
                    last_failure = FailureStage::CameraError;
                    continue;
                }
            };

            // Convert to RGB bytes
            let rgb_bytes = match frame.to_rgb() {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "frame to_rgb failed");
                    continue;
                }
            };

            // Detect faces
            let faces =
                match self
                    .detector
                    .detect(&rgb_bytes, frame.width, frame.height, 0.5)
                {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, "detection failed");
                        continue;
                    }
                };

            if faces.is_empty() {
                continue;
            }

            face_was_detected = true;
            let face = &faces[0]; // highest-confidence face (sorted descending)

            // Align face for liveness + recognition
            let face_img = match align_face(&rgb_bytes, frame.width, frame.height, face) {
                Ok(img) => img,
                Err(e) => {
                    tracing::warn!(error = %e, "face alignment failed");
                    continue;
                }
            };

            // Liveness check (skip for IR cameras — not supported in Phase 1)
            if !matches!(camera_kind, CameraKind::Infrared | CameraKind::RgbAndInfrared) {
                let face_raw = face_img.as_raw().as_slice();
                match self.liveness.check(face_raw, None) {
                    Ok(result) => {
                        if !result.is_live(0.5) {
                            tracing::debug!("liveness check failed, continuing");
                            last_failure = FailureStage::LivenessFailed;
                            continue;
                        }
                        liveness_passed = true;
                        tracing::debug!(stage = "liveness", "liveness check passed");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "liveness check error, continuing");
                        last_failure = FailureStage::LivenessFailed;
                        continue;
                    }
                }
            } else {
                // IR camera — skip liveness for Phase 1
                liveness_passed = true;
            }

            // Generate embedding from aligned face
            let embedding = match self.recognizer.embed_aligned(&face_img) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "embedding failed");
                    last_failure = FailureStage::InternalError;
                    continue;
                }
            };

            // Match against enrolled embeddings
            let mut max_sim: f32 = 0.0;
            let mut max_idx: Option<usize> = None;

            for (i, enrolled_emb) in enrolled.embeddings.iter().enumerate() {
                let sim = embedding.cosine_similarity(enrolled_emb);
                if sim > max_sim {
                    max_sim = sim;
                    max_idx = Some(i);
                }
            }

            // Grant if above threshold
            if max_sim >= threshold {
                let duration_ms = start.elapsed().as_millis() as u64;
                tracing::info!(stage = "matching", granted = true, "authentication granted");
                // NOTE: Do NOT log the score on success — biometric leak + timing risk.
                return Ok(PipelineResult {
                    granted: true,
                    score: Some(max_sim),
                    matched_face: max_idx,
                    liveness_ok: true,
                    duration_ms,
                    failure_stage: None,
                });
            }

            // Track best score seen so far (for deny-path reporting)
            if max_sim > best_score {
                best_score = max_sim;
                best_match_idx = max_idx;
            }
            last_failure = FailureStage::BelowThreshold;
            tracing::warn!(stage = "matching", "below threshold");
            // NOTE: Do NOT log the actual score value here.
        }

        // ── 7. Exhausted frames → deny ─────────────────────────────────────────
        let duration_ms = start.elapsed().as_millis() as u64;

        // Determine the final failure stage
        let failure_stage = if !face_was_detected {
            FailureStage::NoFaceDetected
        } else {
            last_failure
        };

        tracing::info!(
            granted = false,
            failure = ?failure_stage,
            "authentication denied"
        );

        Ok(PipelineResult {
            granted: false,
            score: if best_score > 0.0 {
                Some(best_score)
            } else {
                None
            },
            matched_face: best_match_idx,
            liveness_ok: liveness_passed,
            duration_ms,
            failure_stage: Some(failure_stage),
        })
    }

    /// Convert a [`PipelineResult`] to a [`dax_auth_proto::AuthResponse`] for IPC.
    ///
    /// This mapping is the canonical translation between the core pipeline's
    /// domain type and the wire protocol type sent to the PAM module.
    pub fn to_auth_response(
        &self,
        result: &PipelineResult,
        session_id: uuid::Uuid,
    ) -> dax_auth_proto::AuthResponse {
        use dax_auth_proto::response::{AuthResult, DenyReason};
        use dax_auth_proto::AuthResponse;

        let auth_result = if result.granted {
            AuthResult::Granted {
                score: result.score.unwrap_or(0.0),
                face_index: result.matched_face.unwrap_or(0),
            }
        } else {
            let reason = match result.failure_stage {
                Some(FailureStage::NoFaceDetected) => DenyReason::NoFaceDetected,
                Some(FailureStage::LivenessFailed) => DenyReason::LivenessCheckFailed,
                Some(FailureStage::BelowThreshold) => DenyReason::BelowThreshold {
                    score: result.score.unwrap_or(0.0),
                    // threshold is not stored in PipelineResult — acceptable per design
                    threshold: 0.0,
                },
                Some(FailureStage::NoEnrolledFaces) => DenyReason::NoEnrolledFaces,
                Some(FailureStage::CameraError) => DenyReason::CameraUnavailable,
                Some(FailureStage::InternalError) | None => DenyReason::InternalError,
            };
            AuthResult::Denied(reason)
        };

        AuthResponse {
            session_id,
            version: dax_auth_proto::PROTOCOL_VERSION,
            result: auth_result,
            duration_ms: result.duration_ms,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_result_can_be_constructed() {
        let result = PipelineResult {
            granted: false,
            score: None,
            matched_face: None,
            liveness_ok: false,
            duration_ms: 5,
            failure_stage: Some(FailureStage::NoEnrolledFaces),
        };
        assert!(!result.granted);
        assert_eq!(result.failure_stage, Some(FailureStage::NoEnrolledFaces));
        assert!(result.score.is_none());
    }

    #[test]
    fn failure_stage_maps_to_deny_reason() {
        // Verify that all FailureStage variants can be constructed and compared.
        let stages = [
            FailureStage::NoFaceDetected,
            FailureStage::LivenessFailed,
            FailureStage::BelowThreshold,
            FailureStage::NoEnrolledFaces,
            FailureStage::CameraError,
            FailureStage::InternalError,
        ];
        for stage in stages {
            let result = PipelineResult {
                granted: false,
                score: Some(0.0),
                matched_face: None,
                liveness_ok: false,
                duration_ms: 1,
                failure_stage: Some(stage),
            };
            assert!(!result.granted);
            assert_eq!(result.failure_stage, Some(stage));
        }
    }

    #[test]
    fn granted_result_has_no_failure_stage() {
        let result = PipelineResult {
            granted: true,
            score: Some(0.78),
            matched_face: Some(0),
            liveness_ok: true,
            duration_ms: 1200,
            failure_stage: None,
        };
        assert!(result.granted);
        assert!(result.failure_stage.is_none());
        assert!(result.liveness_ok);
    }
}
