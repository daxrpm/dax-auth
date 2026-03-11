//! Daemon configuration — wraps [`CoreConfig`] with daemon-specific settings.
//!
//! Loads from `/etc/dax-auth/config.toml`. Uses the `config` crate for
//! layered configuration (file + environment variable overrides via `DAX_AUTH_*`).
//!
//! ## Config file structure
//!
//! ```toml
//! [security]
//! mode = "secure"          # or "paranoid"
//! max_attempts = 3
//! auth_timeout_secs = 30
//!
//! [models]
//! dir = "/var/lib/dax-auth/models"
//! detection_model  = "retinaface_10g.onnx"
//! recognition_model = "arcface_r100.onnx"
//! liveness_model   = "minifasnetv2.onnx"
//!
//! [camera]
//! fps = 30
//! max_frames = 90
//!
//! [storage]
//! dir = "/var/lib/dax-auth/users"
//!
//! [inference]
//! intra_threads = 0
//!
//! [daemon]
//! socket_path = "/run/dax-auth/daemon.sock"
//! log_level = "info"
//! journald = true
//! ```

use std::path::{Path, PathBuf};

use dax_auth_core::CoreConfig;
use dax_auth_proto::SecurityMode;
use serde::Deserialize;
use tracing::warn;

// ── Raw structs that mirror config.toml exactly ───────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawSecurityConfig {
    #[serde(default = "default_security_mode")]
    mode: String,
    #[serde(default = "default_max_attempts")]
    max_attempts: u32,
    #[serde(default = "default_auth_timeout_secs")]
    auth_timeout_secs: u64,
}

fn default_security_mode() -> String {
    "secure".into()
}
fn default_max_attempts() -> u32 {
    3
}
fn default_auth_timeout_secs() -> u64 {
    30
}

impl Default for RawSecurityConfig {
    fn default() -> Self {
        Self {
            mode: default_security_mode(),
            max_attempts: default_max_attempts(),
            auth_timeout_secs: default_auth_timeout_secs(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawModelsConfig {
    #[serde(default = "default_models_dir")]
    dir: String,
    #[serde(default = "default_detection_model")]
    detection_model: String,
    #[serde(default = "default_recognition_model")]
    recognition_model: String,
    #[serde(default = "default_liveness_model")]
    liveness_model: String,
}

fn default_models_dir() -> String {
    "/var/lib/dax-auth/models".into()
}
fn default_detection_model() -> String {
    "retinaface_10g.onnx".into()
}
fn default_recognition_model() -> String {
    "arcface_r100.onnx".into()
}
fn default_liveness_model() -> String {
    "minifasnetv2.onnx".into()
}

impl Default for RawModelsConfig {
    fn default() -> Self {
        Self {
            dir: default_models_dir(),
            detection_model: default_detection_model(),
            recognition_model: default_recognition_model(),
            liveness_model: default_liveness_model(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawCameraConfig {
    #[serde(default = "default_fps")]
    fps: u32,
    #[serde(default = "default_max_frames")]
    max_frames: u32,
}

fn default_fps() -> u32 {
    30
}
fn default_max_frames() -> u32 {
    90
}

impl Default for RawCameraConfig {
    fn default() -> Self {
        Self {
            fps: default_fps(),
            max_frames: default_max_frames(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawStorageConfig {
    #[serde(default = "default_storage_dir")]
    dir: String,
}

fn default_storage_dir() -> String {
    "/var/lib/dax-auth/users".into()
}

impl Default for RawStorageConfig {
    fn default() -> Self {
        Self {
            dir: default_storage_dir(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawInferenceConfig {
    #[serde(default)]
    intra_threads: u32,
}

impl Default for RawInferenceConfig {
    fn default() -> Self {
        Self { intra_threads: 0 }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawDaemonSection {
    #[serde(default = "default_socket_path")]
    socket_path: String,
    #[serde(default = "default_log_level")]
    log_level: String,
    #[serde(default = "default_journald")]
    journald: bool,
}

fn default_socket_path() -> String {
    "/run/dax-auth/daemon.sock".into()
}
fn default_log_level() -> String {
    "info".into()
}
fn default_journald() -> bool {
    true
}

impl Default for RawDaemonSection {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            log_level: default_log_level(),
            journald: default_journald(),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct RawLivenessConfig {
    #[serde(default = "default_liveness_strategy")]
    strategy: String,
    #[serde(default = "default_liveness_threshold")]
    liveness_threshold: f32,
}

fn default_liveness_strategy() -> String {
    "auto".into()
}
fn default_liveness_threshold() -> f32 {
    0.5
}

/// Raw top-level config — mirrors `config.toml` structure exactly.
#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    security: RawSecurityConfig,
    #[serde(default)]
    liveness: RawLivenessConfig,
    #[serde(default)]
    camera: RawCameraConfig,
    #[serde(default)]
    models: RawModelsConfig,
    #[serde(default)]
    storage: RawStorageConfig,
    #[serde(default)]
    inference: RawInferenceConfig,
    #[serde(default)]
    daemon: RawDaemonSection,
}

// ── Public DaemonConfig ───────────────────────────────────────────────────────

/// Full daemon configuration: core ML pipeline settings + daemon-specific fields.
///
/// Loaded from `/etc/dax-auth/config.toml` with environment variable overrides
/// (`DAX_AUTH_*`). Falls back to safe defaults when no config file is present.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Core ML pipeline configuration (models, thresholds, camera settings).
    pub core: CoreConfig,

    /// Path to the Unix domain socket.
    pub socket_path: PathBuf,

    /// Directory where encrypted face embeddings are stored.
    pub storage_dir: PathBuf,

    /// Log level string (e.g. `"info"`, `"debug"`).
    pub log_level: String,

    /// Whether to prefer journald over stderr for logging.
    pub journald: bool,

    /// Security mode for authentication.
    pub security_mode: SecurityMode,

    /// Maximum authentication attempts before falling back to password.
    pub max_attempts: u32,

    /// Timeout per authentication attempt, in seconds.
    pub auth_timeout_secs: u64,
}

/// Default config path.
const DEFAULT_CONFIG_PATH: &str = "/etc/dax-auth/config.toml";

impl DaemonConfig {
    /// Load from the default path (`/etc/dax-auth/config.toml`).
    ///
    /// Missing file is treated as "use defaults". Any other error is propagated.
    ///
    /// # Errors
    /// Returns an error if the config file exists but cannot be parsed.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from_path(Path::new(DEFAULT_CONFIG_PATH))
    }

    /// Load from an explicit path (useful for testing or overrides).
    ///
    /// # Errors
    /// Returns an error if the config file exists but cannot be parsed.
    pub fn load_from_path(path: &Path) -> anyhow::Result<Self> {
        let raw: RawConfig = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                anyhow::anyhow!("failed to parse config at {}: {e}", path.display())
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    path = %path.display(),
                    "config file not found — using all defaults"
                );
                RawConfig::default()
            }
            Err(e) => {
                anyhow::bail!("failed to read config at {}: {e}", path.display());
            }
        };

        let security_mode = parse_security_mode(&raw.security.mode);
        let core = build_core_config(&raw);

        Ok(Self {
            core,
            socket_path: PathBuf::from(&raw.daemon.socket_path),
            storage_dir: PathBuf::from(&raw.storage.dir),
            log_level: raw.daemon.log_level,
            journald: raw.daemon.journald,
            security_mode,
            max_attempts: raw.security.max_attempts,
            auth_timeout_secs: raw.security.auth_timeout_secs,
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse a security mode string, defaulting to `Secure` on unknown values.
fn parse_security_mode(s: &str) -> SecurityMode {
    match s {
        "paranoid" => SecurityMode::Paranoid,
        _ => SecurityMode::Secure,
    }
}

/// Build a [`CoreConfig`] from raw deserialized values.
fn build_core_config(raw: &RawConfig) -> CoreConfig {
    CoreConfig {
        models_dir: PathBuf::from(&raw.models.dir),
        detector_model: raw.models.detection_model.clone(),
        recognizer_model: raw.models.recognition_model.clone(),
        anti_spoof_model: raw.models.liveness_model.clone(),
        max_frames: raw.camera.max_frames,
        capture_fps: raw.camera.fps,
        execution_provider: dax_auth_core::config::ExecutionProviderConfig {
            cpu_threads: raw.inference.intra_threads,
            ..Default::default()
        },
        ..Default::default()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let config = DaemonConfig::load_from_path(Path::new("/nonexistent/path/config.toml"))
            .expect("missing file should use defaults");
        assert_eq!(
            config.socket_path.to_str(),
            Some("/run/dax-auth/daemon.sock")
        );
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.core.thresholds.secure, 0.65);
        assert_eq!(config.core.thresholds.paranoid, 0.72);
    }

    #[test]
    fn security_mode_parsed_correctly() {
        assert_eq!(parse_security_mode("paranoid"), SecurityMode::Paranoid);
        assert_eq!(parse_security_mode("secure"), SecurityMode::Secure);
        assert_eq!(parse_security_mode("unknown"), SecurityMode::Secure);
    }

    #[test]
    fn load_from_toml_string() {
        let toml = r#"
[security]
mode = "paranoid"
max_attempts = 5
auth_timeout_secs = 60

[models]
dir = "/tmp/models"
detection_model = "detect.onnx"
recognition_model = "recog.onnx"
liveness_model = "liveness.onnx"

[camera]
fps = 15
max_frames = 45

[storage]
dir = "/tmp/users"

[inference]
intra_threads = 4

[daemon]
socket_path = "/tmp/dax.sock"
log_level = "debug"
journald = false
"#;
        let raw: RawConfig = toml::from_str(toml).expect("parse toml");
        let cfg = DaemonConfig {
            core: build_core_config(&raw),
            socket_path: PathBuf::from(&raw.daemon.socket_path),
            storage_dir: PathBuf::from(&raw.storage.dir),
            log_level: raw.daemon.log_level.clone(),
            journald: raw.daemon.journald,
            security_mode: parse_security_mode(&raw.security.mode),
            max_attempts: raw.security.max_attempts,
            auth_timeout_secs: raw.security.auth_timeout_secs,
        };
        assert_eq!(cfg.security_mode, SecurityMode::Paranoid);
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.core.max_frames, 45);
        assert_eq!(cfg.core.models_dir, PathBuf::from("/tmp/models"));
        assert!(!cfg.journald);
    }
}
