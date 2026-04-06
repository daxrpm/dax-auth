//! Core pipeline configuration.

use crate::CoreError;
use dax_auth_proto::SecurityMode;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Private sectioned structs (mirror config.toml layout) ────────────────────
// Used only in CoreConfig::load() to deserialize the sectioned TOML file.
// The public CoreConfig remains flat for ease of use in tests and code.

#[derive(Debug, Deserialize, Default)]
struct RawFileModels {
    #[serde(default = "default_models_dir_str")]
    dir: String,
    #[serde(default = "default_detector_model_str")]
    detection_model: String,
    #[serde(default = "default_recognizer_model_str")]
    recognition_model: String,
    #[serde(default = "default_anti_spoof_model_str")]
    liveness_model: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawFileCamera {
    #[serde(default = "default_capture_fps")]
    fps: u32,
    #[serde(default = "default_max_frames")]
    max_frames: u32,
}

#[derive(Debug, Deserialize, Default)]
struct RawFileInference {
    #[serde(default)]
    intra_threads: u32,
}

/// Top-level sectioned config — mirrors `config.toml` structure.
/// Unknown fields are silently ignored (no `deny_unknown_fields`).
#[derive(Debug, Deserialize, Default)]
struct RawFileConfig {
    #[serde(default)]
    models: RawFileModels,
    #[serde(default)]
    camera: RawFileCamera,
    #[serde(default)]
    inference: RawFileInference,
}

fn default_models_dir_str() -> String {
    "/var/lib/dax-auth/models".into()
}
fn default_detector_model_str() -> String {
    "det_10g.onnx".into()
}
fn default_recognizer_model_str() -> String {
    "w600k_r50.onnx".into()
}
fn default_anti_spoof_model_str() -> String {
    "minifasnet_v2.onnx".into()
}
fn default_capture_fps() -> u32 {
    15
}
fn default_max_frames() -> u32 {
    30
}

/// Configuration for the inference pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// Directory where ONNX model files are stored.
    pub models_dir: PathBuf,

    /// Face detection model filename (default: `det_10g.onnx`).
    pub detector_model: String,

    /// Face recognition model filename (default: `w600k_r50.onnx`).
    pub recognizer_model: String,

    /// Anti-spoofing model filename (default: `minifasnet_v2.onnx`).
    pub anti_spoof_model: String,

    /// Execution provider configuration.
    pub execution_provider: ExecutionProviderConfig,

    /// Authentication thresholds per security mode.
    pub thresholds: ThresholdConfig,

    /// Maximum number of frames to attempt before giving up.
    pub max_frames: u32,

    /// Frames per second to capture (default: 15).
    pub capture_fps: u32,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            models_dir: PathBuf::from("/var/lib/dax-auth/models"),
            // InsightFace buffalo_l pack filenames (SCRFD + WebFace600K)
            detector_model: "det_10g.onnx".into(),
            recognizer_model: "w600k_r50.onnx".into(),
            anti_spoof_model: "minifasnet_v2.onnx".into(),
            execution_provider: ExecutionProviderConfig::default(),
            thresholds: ThresholdConfig::default(),
            max_frames: 30,
            capture_fps: 15,
        }
    }
}

impl CoreConfig {
    /// Load configuration from a TOML file at `path`.
    ///
    /// If the file does not exist, returns [`CoreConfig::default_config()`] and
    /// logs a warning. Any other I/O error or parse error is returned as
    /// [`CoreError::ConfigLoad`].
    ///
    /// # Errors
    /// Returns [`CoreError::ConfigLoad`] if the file exists but cannot be read
    /// or contains invalid TOML.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let raw: RawFileConfig = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                CoreError::ConfigLoad(format!("failed to parse {}: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    path = %path.display(),
                    "config file not found — using defaults"
                );
                return Ok(Self::default_config());
            }
            Err(e) => {
                return Err(CoreError::ConfigLoad(format!(
                    "failed to read {}: {e}",
                    path.display()
                )))
            }
        };

        Ok(Self {
            models_dir: PathBuf::from(&raw.models.dir),
            detector_model: raw.models.detection_model,
            recognizer_model: raw.models.recognition_model,
            anti_spoof_model: raw.models.liveness_model,
            max_frames: raw.camera.max_frames,
            capture_fps: raw.camera.fps,
            execution_provider: ExecutionProviderConfig {
                cpu_threads: raw.inference.intra_threads,
                ..Default::default()
            },
            thresholds: ThresholdConfig::default(),
        })
    }

    /// Returns a [`CoreConfig`] populated with production-ready default values.
    ///
    /// Used when no config file is present.
    #[must_use]
    pub fn default_config() -> Self {
        Self::default()
    }

    /// Returns the cosine similarity threshold for the given security mode.
    #[must_use]
    pub fn threshold_for(&self, mode: SecurityMode) -> f32 {
        match mode {
            SecurityMode::Secure => self.thresholds.secure,
            SecurityMode::Paranoid => self.thresholds.paranoid,
        }
    }
}

/// Execution provider priority configuration.
///
/// The daemon tries providers in order and falls back gracefully.
/// CPU is always the final fallback and cannot be disabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionProviderConfig {
    /// Enable ROCm (AMD GPU) if available.
    pub try_rocm: bool,
    /// Enable CUDA (NVIDIA GPU) if available.
    pub try_cuda: bool,
    /// Enable OpenVINO (Intel) if available.
    pub try_openvino: bool,
    /// Number of CPU threads for ONNX Runtime (0 = auto-detect).
    pub cpu_threads: u32,
}

impl Default for ExecutionProviderConfig {
    fn default() -> Self {
        Self {
            try_rocm: true,
            try_cuda: true,
            try_openvino: true,
            cpu_threads: 0, // auto
        }
    }
}

/// Cosine similarity thresholds per security mode.
///
/// These are calibrated to approximate Windows Hello FAR ≤ 1e-4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdConfig {
    /// Threshold for `secure` mode (≈ Windows Hello).
    pub secure: f32,
    /// Threshold for `paranoid` mode (stricter).
    pub paranoid: f32,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            secure: 0.65,
            paranoid: 0.72,
        }
    }
}
