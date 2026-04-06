//! ONNX model metadata, download manifest, and session registry.
//!
//! All models used are open source with permissive licenses compatible with GPL-3.0.

use crate::{config::CoreConfig, CoreError};
use ort::session::{builder::GraphOptimizationLevel, Session};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::info;

/// Metadata about a supported ONNX model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier used in config files.
    pub id: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// License of the model weights.
    pub license: &'static str,
    /// Expected filename in `models_dir`.
    pub filename: &'static str,
    /// SHA-256 checksum for integrity verification.
    pub sha256: Option<&'static str>,
    /// Download URL (for the `dax-auth download-models` command).
    pub download_url: Option<&'static str>,
    /// ONNX opset version.
    pub opset: u32,
    /// Input size for this model [width, height].
    pub input_size: [u32; 2],
}

/// All supported detection models.
pub const DETECTION_MODELS: &[ModelInfo] = &[ModelInfo {
    id: "det_10g",
    description: "InsightFace SCRFD-10G — efficient face detector, 10 GFLOPs at 640×640",
    license: "MIT",
    filename: "det_10g.onnx",
    sha256: Some("5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"),
    download_url: Some(
        "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip",
    ),
    opset: 11,
    input_size: [640, 640],
}];

/// All supported recognition models.
pub const RECOGNITION_MODELS: &[ModelInfo] = &[ModelInfo {
    id: "w600k_r50",
    description: "ArcFace WebFace600K ResNet50 — high accuracy face recognition",
    license: "Apache-2.0",
    filename: "w600k_r50.onnx",
    sha256: Some("4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"),
    download_url: Some(
        "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip",
    ),
    opset: 11,
    input_size: [112, 112],
}];

/// All supported anti-spoofing models.
pub const ANTI_SPOOF_MODELS: &[ModelInfo] = &[ModelInfo {
    id: "minifasnet_v2",
    description: "MiniFASNetV2 — silent face anti-spoofing via Fourier spectrum (Apache 2.0)",
    license: "Apache-2.0",
    filename: "minifasnet_v2.onnx",
    sha256: None,
    download_url: None, // needs conversion from PyTorch checkpoint
    opset: 11,
    input_size: [80, 80],
}];

/// Holds all three loaded ONNX inference sessions.
///
/// Loaded eagerly at daemon startup — if any model is missing or corrupt,
/// the daemon fails immediately rather than on the first auth attempt.
pub struct ModelRegistry {
    /// Loaded SCRFD detection session.
    pub detector: Session,
    /// Loaded ArcFace recognition session.
    pub recognizer: Session,
    /// Loaded MiniFASNetV2 anti-spoofing session.
    /// `None` when liveness strategy is "disabled" or the model file is absent.
    pub anti_spoof: Option<Session>,
}

impl ModelRegistry {
    /// Load all three ONNX sessions from the configured model directory.
    ///
    /// Verifies SHA-256 checksums when available (`ModelInfo::sha256` is `Some`).
    /// Tries execution providers in priority order: ROCm → CUDA → OpenVINO → CPU.
    ///
    /// # Errors
    /// Returns [`CoreError::ModelNotFound`] if any model file is missing.
    /// Returns [`CoreError::ModelTampered`] if a SHA-256 checksum fails.
    /// Returns [`CoreError::Inference`] if the ONNX session cannot be built.
    pub fn load(config: &CoreConfig) -> Result<Self, CoreError> {
        let ep_config = &config.execution_provider;

        // Build the ordered list of EP dispatches based on config flags
        let eps = build_ep_list(ep_config);

        let threads = config.execution_provider.cpu_threads;

        // ── Detector (RetinaFace) ─────────────────────────────────────────────
        let detector_path = config.models_dir.join(&config.detector_model);
        check_file_exists(&detector_path)?;
        // SHA-256 check is enforced whenever the model metadata includes a hash.
        if let Some(info) = DETECTION_MODELS
            .iter()
            .find(|m| m.filename == config.detector_model)
        {
            if let Some(expected_hash) = info.sha256 {
                verify_sha256(&detector_path, expected_hash)?;
            }
        }
        info!(
            path = %detector_path.display(),
            "loading detection model (RetinaFace)"
        );
        let detector = load_onnx_session(&detector_path, &eps, threads)?;
        info!(
            inputs = ?detector.inputs().iter().map(|i| i.name()).collect::<Vec<_>>(),
            outputs = ?detector.outputs().iter().map(|o| o.name()).collect::<Vec<_>>(),
            "detection model loaded"
        );

        // ── Recognizer (ArcFace R100) ─────────────────────────────────────────
        let recognizer_path = config.models_dir.join(&config.recognizer_model);
        check_file_exists(&recognizer_path)?;
        if let Some(info) = RECOGNITION_MODELS
            .iter()
            .find(|m| m.filename == config.recognizer_model)
        {
            if let Some(expected_hash) = info.sha256 {
                verify_sha256(&recognizer_path, expected_hash)?;
            }
        }
        info!(
            path = %recognizer_path.display(),
            "loading recognition model (ArcFace R100)"
        );
        let recognizer = load_onnx_session(&recognizer_path, &eps, threads)?;
        info!(
            inputs = ?recognizer.inputs().iter().map(|i| i.name()).collect::<Vec<_>>(),
            outputs = ?recognizer.outputs().iter().map(|o| o.name()).collect::<Vec<_>>(),
            "recognition model loaded"
        );

        // ── Anti-spoof (MiniFASNetV2) — optional ─────────────────────────────
        // If the model filename is "disabled" or the file doesn't exist,
        // skip loading. The pipeline will pass liveness automatically.
        let anti_spoof_path = config.models_dir.join(&config.anti_spoof_model);
        let anti_spoof = if config.anti_spoof_model == "disabled" {
            info!("anti-spoofing disabled via config");
            None
        } else if !anti_spoof_path.exists() {
            tracing::warn!(
                path = %anti_spoof_path.display(),
                "anti-spoofing model not found — liveness check will be skipped (WARNING: reduced security)"
            );
            None
        } else {
            if let Some(info) = ANTI_SPOOF_MODELS
                .iter()
                .find(|m| m.filename == config.anti_spoof_model)
            {
                if let Some(expected_hash) = info.sha256 {
                    verify_sha256(&anti_spoof_path, expected_hash)?;
                }
            }
            info!(path = %anti_spoof_path.display(), "loading anti-spoofing model (MiniFASNetV2)");
            let session = load_onnx_session(&anti_spoof_path, &eps, threads)?;
            info!(
                inputs = ?session.inputs().iter().map(|i| i.name()).collect::<Vec<_>>(),
                outputs = ?session.outputs().iter().map(|o| o.name()).collect::<Vec<_>>(),
                "anti-spoofing model loaded"
            );
            Some(session)
        };

        Ok(Self {
            detector,
            recognizer,
            anti_spoof,
        })
    }
}

/// Verify that a model file exists and is accessible.
fn check_file_exists(path: &Path) -> Result<(), CoreError> {
    if !path.exists() {
        return Err(CoreError::ModelNotFound {
            path: path.display().to_string(),
        });
    }
    Ok(())
}

/// Verify the SHA-256 hash of a model file.
///
/// Returns [`CoreError::ModelTampered`] if the computed hash does not match `expected`.
fn verify_sha256(path: &Path, expected: &str) -> Result<(), CoreError> {
    use sha2::{Digest, Sha256};

    let bytes = std::fs::read(path).map_err(|_| CoreError::ModelNotFound {
        path: path.display().to_string(),
    })?;
    let hash = Sha256::digest(&bytes);
    let hex = format!("{hash:x}");
    if hex != expected {
        return Err(CoreError::ModelTampered {
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// Build the ONNX Runtime execution provider dispatch list from config.
///
/// CPU is always appended last as the unconditional fallback.
#[allow(unused_variables)] // ep_config fields are only accessed under GPU feature flags
fn build_ep_list(
    ep_config: &crate::config::ExecutionProviderConfig,
) -> Vec<ort::execution_providers::ExecutionProviderDispatch> {
    let mut eps: Vec<ort::execution_providers::ExecutionProviderDispatch> = Vec::new();

    #[cfg(feature = "rocm")]
    if ep_config.try_rocm {
        info!("adding ROCm execution provider");
        eps.push(ort::execution_providers::ROCmExecutionProvider::default().build());
    }

    #[cfg(feature = "cuda")]
    if ep_config.try_cuda {
        info!("adding CUDA execution provider");
        eps.push(ort::execution_providers::CUDAExecutionProvider::default().build());
    }

    #[cfg(feature = "openvino")]
    if ep_config.try_openvino {
        info!("adding OpenVINO execution provider");
        eps.push(ort::execution_providers::OpenVINOExecutionProvider::default().build());
    }

    // CPU is always available — no feature gate needed
    eps.push(ort::execution_providers::CPUExecutionProvider::default().build());

    eps
}

/// Build and commit an ONNX Runtime session for a model file.
///
/// The session uses optimization level 3 (full graph optimizations) and
/// configures intra-op parallelism. If `cpu_threads` is `0`, the thread
/// count is auto-detected from available CPU cores.
fn load_onnx_session(
    path: &Path,
    eps: &[ort::execution_providers::ExecutionProviderDispatch],
    cpu_threads: u32,
) -> Result<Session, CoreError> {
    let threads: usize = if cpu_threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
    } else {
        cpu_threads as usize
    };

    Session::builder()
        .map_err(|e| CoreError::Inference(e.to_string()))?
        // ORT_ENABLE_ALL (99) — use All instead of Level3 (ORT_ENABLE_LAYOUT=3)
        // which is rejected by ORT 1.21 as an invalid optimization level.
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .with_intra_threads(threads)
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .with_execution_providers(eps.to_vec())
        .map_err(|e| CoreError::Inference(e.to_string()))?
        .commit_from_file(path)
        .map_err(|e| CoreError::Inference(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CoreConfig;
    use std::path::PathBuf;

    #[test]
    fn model_not_found_returns_correct_error() {
        let mut config = CoreConfig::default();
        config.models_dir = PathBuf::from("/nonexistent/path");
        let result = ModelRegistry::load(&config);
        assert!(
            matches!(result, Err(CoreError::ModelNotFound { .. })),
            "expected ModelNotFound error"
        );
    }
}
