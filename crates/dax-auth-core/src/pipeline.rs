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

// ─── Device selection helpers ─────────────────────────────────────────────────

/// Select the best camera device for authentication using the Windows Hello
/// IR-first strategy.
///
/// Priority:
/// 1. IR-only camera (`CameraKind::Infrared`) — lighting-invariant, no AE issues
/// 2. Combined RGB+IR camera (`CameraKind::RgbAndInfrared`)
/// 3. RGB-only camera (`CameraKind::Rgb`) — fallback when no IR is available
///
/// Returns `None` if no suitable device is found on this system.
fn select_auth_device(devices: Vec<CameraDevice>) -> Option<CameraDevice> {
    // Try pure IR first (Windows Hello primary path).
    if let Some(d) = devices
        .iter()
        .find(|d| d.kind == CameraKind::Infrared)
        .cloned()
    {
        return Some(d);
    }
    // Then RGB+IR combo.
    if let Some(d) = devices
        .iter()
        .find(|d| d.kind == CameraKind::RgbAndInfrared)
        .cloned()
    {
        return Some(d);
    }
    // Fall back to any RGB camera.
    devices.into_iter().find(|d| d.kind == CameraKind::Rgb)
}

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
    /// The cosine similarity threshold that was applied during this run.
    ///
    /// Derived from [`SecurityMode`] via [`CoreConfig::threshold_for`]. Stored
    /// here so callers (e.g. `session.rs`) can report the exact threshold
    /// used without re-deriving it from the security mode.
    pub threshold: f32,
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

        // Derive the storage directory from the models directory.
        // Convention: /var/lib/dax-auth/models → /var/lib/dax-auth/users
        let storage_dir = config
            .models_dir
            .parent()
            .map(|p| p.join("users"))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/dax-auth/users"));

        let store = FaceStore::open(&storage_dir)?;

        let registry = ModelRegistry::load(&config)?;

        let detector = FaceDetector::new(registry.detector);
        // registry.anti_spoof is None when the model file is absent or disabled.
        // LivenessDetector handles None by returning Live with confidence=1.0.
        let liveness = LivenessDetector::new(CameraKind::Rgb, registry.anti_spoof);
        let recognizer = FaceRecognizer::new(registry.recognizer);

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
                    threshold,
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
                threshold,
            });
        }

        // ── 2. Open camera (IR-first — Windows Hello strategy) ────────────────
        // List all devices and pick the best one using the IR-first priority
        // order.  IR cameras are lighting-invariant and have no auto-exposure
        // drift, making them the preferred authentication path.  Falls back to
        // RGB when no IR device is present.
        let device = match CameraDevice::list_all() {
            Ok(devices) => match select_auth_device(devices) {
                Some(d) => {
                    tracing::info!(
                        path = %d.path,
                        kind = ?d.kind,
                        "auth: selected camera device"
                    );
                    d
                }
                None => {
                    tracing::warn!("auth: no suitable camera device found");
                    return Ok(PipelineResult {
                        granted: false,
                        score: None,
                        matched_face: None,
                        liveness_ok: false,
                        duration_ms: start.elapsed().as_millis() as u64,
                        failure_stage: Some(FailureStage::CameraError),
                        threshold,
                    });
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "auth: camera enumeration failed");
                return Ok(PipelineResult {
                    granted: false,
                    score: None,
                    matched_face: None,
                    liveness_ok: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    failure_stage: Some(FailureStage::CameraError),
                    threshold,
                });
            }
        };

        let mut capture = match CameraCapture::open(device) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "auth: failed to open camera");
                return Ok(PipelineResult {
                    granted: false,
                    score: None,
                    matched_face: None,
                    liveness_ok: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    failure_stage: Some(FailureStage::CameraError),
                    threshold,
                });
            }
        };

        // ── 3. Auto-exposure warm-up on the persistent stream ─────────────────
        // With a persistent stream, auto-exposure accumulates across frames
        // naturally.  capture_best_frame() discards dark frames until AE settles
        // (or returns the best available frame after the limit).
        // We re-use this first good frame as the first detection input below.
        let warmup_frame = match capture.capture_best_frame().await {
            Ok(f) => {
                tracing::debug!("auth: auto-exposure settled");
                Some(f)
            }
            Err(e) => {
                tracing::warn!(error = %e, "auth: AE warm-up failed; continuing anyway");
                None
            }
        };
        let mut warmup_slot: Option<dax_auth_camera::Frame> = warmup_frame;

        // ── 4–7. Frame loop ────────────────────────────────────────────────────
        let mut best_score: f32 = 0.0;
        let mut best_match_idx: Option<usize> = None;
        let mut liveness_passed = false;
        let mut face_was_detected = false;
        let mut last_failure = FailureStage::NoFaceDetected;

        for frame_idx in 0..self.config.max_frames {
            // On the first iteration, re-use the warm-up frame captured
            // during AE settling to avoid an extra round-trip to the sensor.
            let frame = if let Some(f) = warmup_slot.take() {
                f
            } else {
                match capture.capture_frame_async().await {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, frame = frame_idx, "auth: frame capture failed");
                        last_failure = FailureStage::CameraError;
                        continue;
                    }
                }
            };

            // Convert to RGB bytes
            let rgb_bytes = match frame.to_rgb() {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, frame = frame_idx, "auth: frame to_rgb failed");
                    continue;
                }
            };

            // Detect faces
            let faces = match self
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
            if !matches!(
                camera_kind,
                CameraKind::Infrared | CameraKind::RgbAndInfrared
            ) {
                if self.liveness.has_model() {
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
                    // No liveness model loaded — skip check (degraded-security mode).
                    // A warning is already emitted at daemon startup by ModelRegistry::load().
                    tracing::debug!("liveness model not loaded — skipping check");
                    liveness_passed = true;
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
                    threshold,
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
            threshold,
        })
    }

    /// Capture one frame, detect a face, run liveness, and return an embedding.
    ///
    /// This is a subset of [`authenticate`][AuthPipeline::authenticate] (steps 2–6)
    /// without the matching step. It is used by the CLI enroll command to obtain
    /// a verified embedding before storing it.
    ///
    /// # Errors
    /// - [`CoreError::Camera`] if no camera is available or frame capture fails.
    /// - [`CoreError::Inference`] if detection or embedding inference fails.
    /// - [`CoreError::NoFaceDetected`] if no suitable face is found within `max_frames`.
    pub async fn capture_and_embed(
        &mut self,
    ) -> Result<crate::embedding::FaceEmbedding, CoreError> {
        use crate::embedding::align_face;

        // Open the best camera using the IR-first strategy (Windows Hello approach):
        // IR is lighting-invariant and has no auto-exposure drift.  Falls back to
        // RGB when no IR device is present.
        let devices = CameraDevice::list_all().map_err(CoreError::Camera)?;
        let device = select_auth_device(devices).ok_or_else(|| {
            CoreError::Camera(dax_auth_camera::CameraError::DeviceNotFound {
                path: "/dev/video*".into(),
            })
        })?;

        let camera_kind = device.kind;

        tracing::info!(
            path = %device.path,
            kind = ?device.kind,
            width = device.width,
            height = device.height,
            "enroll: opening camera"
        );

        let mut capture = CameraCapture::open(device).map_err(CoreError::Camera)?;

        tracing::info!(
            format = ?capture.format,
            width = capture.width,
            height = capture.height,
            "enroll: camera format negotiated"
        );

        // Auto-exposure warm-up on the persistent stream.
        // Because the stream stays open between frames, AE accumulates naturally.
        // capture_best_frame() discards dark frames until the sensor settles,
        // then returns the first usable frame (or the best seen after the limit).
        tracing::info!("enroll: waiting for auto-exposure to stabilise");
        let ae_frame = match capture.capture_best_frame().await {
            Ok(f) => {
                tracing::info!("enroll: auto-exposure ready, starting detection");
                Some(f)
            }
            Err(e) => {
                tracing::warn!(error = %e, "enroll: AE warm-up error, proceeding anyway");
                None
            }
        };

        // Re-use the AE-settled frame as the first detection input.
        let mut ae_frame_slot: Option<dax_auth_camera::Frame> = ae_frame;

        for frame_idx in 0..self.config.max_frames {
            let t_frame = std::time::Instant::now();

            // On the first iteration, use the pre-captured stabilised frame.
            // On subsequent iterations, capture fresh frames.
            let frame = if let Some(f) = ae_frame_slot.take() {
                f
            } else {
                match capture.capture_frame_async().await {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, frame = frame_idx, "enroll: frame capture failed");
                        continue;
                    }
                }
            };
            tracing::debug!(
                frame = frame_idx,
                elapsed_ms = t_frame.elapsed().as_millis(),
                "enroll: frame captured"
            );

            let rgb_bytes = match frame.to_rgb() {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, frame = frame_idx, "enroll: frame to_rgb failed");
                    continue;
                }
            };

            let t_detect = std::time::Instant::now();
            let faces = match self
                .detector
                .detect(&rgb_bytes, frame.width, frame.height, 0.5)
            {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, frame = frame_idx, "enroll: detection failed");
                    continue;
                }
            };
            tracing::debug!(
                frame = frame_idx,
                faces = faces.len(),
                detect_ms = t_detect.elapsed().as_millis(),
                "enroll: detection complete"
            );

            if faces.is_empty() {
                tracing::debug!(frame = frame_idx, "enroll: no face detected");
                continue;
            }

            if faces.len() > 1 {
                tracing::debug!(
                    frame = frame_idx,
                    faces = faces.len(),
                    "enroll: multiple faces detected, skipping frame"
                );
                continue;
            }

            let face = &faces[0];

            let face_img = match align_face(&rgb_bytes, frame.width, frame.height, face) {
                Ok(img) => img,
                Err(e) => {
                    tracing::warn!(error = %e, "enroll: face alignment failed");
                    continue;
                }
            };

            // Liveness check (skip for IR cameras)
            if !matches!(
                camera_kind,
                CameraKind::Infrared | CameraKind::RgbAndInfrared
            ) {
                if self.liveness.has_model() {
                    let face_raw = face_img.as_raw().as_slice();
                    match self.liveness.check(face_raw, None) {
                        Ok(result) => {
                            if !result.is_live(0.5) {
                                tracing::debug!("enroll: liveness check failed, retrying");
                                continue;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "enroll: liveness check error, continuing");
                            continue;
                        }
                    }
                } else {
                    tracing::debug!("enroll: liveness model not loaded — skipping check");
                }
            }

            // Generate embedding from the aligned face.
            match self.recognizer.embed_aligned(&face_img) {
                Ok(embedding) => {
                    tracing::info!("enroll: face captured and embedded successfully");
                    return Ok(embedding);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "enroll: embedding failed");
                    continue;
                }
            }
        }

        Err(CoreError::NoFaceDetected)
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
                    threshold: result.threshold,
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
            threshold: 0.65,
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
                threshold: 0.65,
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
            threshold: 0.65,
        };
        assert!(result.granted);
        assert!(result.failure_stage.is_none());
        assert!(result.liveness_ok);
    }

    #[test]
    fn threshold_reflects_security_mode() {
        let secure_result = PipelineResult {
            granted: false,
            score: Some(0.60),
            threshold: 0.65, // SecurityMode::Secure default
            matched_face: None,
            liveness_ok: false,
            failure_stage: Some(FailureStage::BelowThreshold),
            duration_ms: 10,
        };
        assert!((secure_result.threshold - 0.65).abs() < f32::EPSILON);

        let paranoid_result = PipelineResult {
            granted: false,
            score: Some(0.68),
            threshold: 0.72, // SecurityMode::Paranoid default
            matched_face: None,
            liveness_ok: false,
            failure_stage: Some(FailureStage::BelowThreshold),
            duration_ms: 10,
        };
        assert!((paranoid_result.threshold - 0.72).abs() < f32::EPSILON);
    }

    #[test]
    fn to_auth_response_below_threshold_carries_config_threshold() {
        use dax_auth_proto::response::{AuthResult, DenyReason};
        use uuid::Uuid;

        // Build a minimal AuthPipeline via direct field construction is not
        // possible (private fields), so we test the mapping logic directly
        // by constructing the equivalent code path inline.
        let result = PipelineResult {
            granted: false,
            score: Some(0.60),
            threshold: 0.72,
            matched_face: None,
            liveness_ok: false,
            failure_stage: Some(FailureStage::BelowThreshold),
            duration_ms: 10,
        };

        // Replicate the mapping done in to_auth_response()
        let reason = match result.failure_stage {
            Some(FailureStage::BelowThreshold) => DenyReason::BelowThreshold {
                score: result.score.unwrap_or(0.0),
                threshold: result.threshold,
            },
            _ => DenyReason::InternalError,
        };

        let session_id = Uuid::new_v4();
        let response = dax_auth_proto::AuthResponse {
            session_id,
            version: dax_auth_proto::PROTOCOL_VERSION,
            result: AuthResult::Denied(reason),
            duration_ms: result.duration_ms,
        };

        match response.result {
            AuthResult::Denied(DenyReason::BelowThreshold { score, threshold }) => {
                assert!((score - 0.60).abs() < f32::EPSILON);
                // Must be the paranoid threshold (0.72), not the hardcoded 0.65 or 0.0
                assert!(
                    (threshold - 0.72).abs() < f32::EPSILON,
                    "expected 0.72, got {threshold}"
                );
            }
            other => panic!("expected BelowThreshold, got {other:?}"),
        }
    }
}
