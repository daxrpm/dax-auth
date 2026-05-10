//! On-disk configuration for system-wide installs.
//!
//! The installer writes `/etc/dax-auth/config.toml` so the PAM
//! module can find the vault, models, and camera index without
//! relying on environment variables (PAM-loaded `.so` files inherit
//! a sanitised environment from the parent process and cannot rely
//! on `DAX_*` vars being set).
//!
//! Per-user overrides live in `$XDG_CONFIG_HOME/dax-auth/config.toml`
//! (defaulting to `~/.config/dax-auth/config.toml`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{RuntimeError, RuntimeResult};

pub const SYSTEM_CONFIG_PATH: &str = "/etc/dax-auth/config.toml";

/// Top-level on-disk configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    #[serde(default)]
    pub camera: CameraConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    pub vault: PathBuf,
    pub detector: PathBuf,
    pub recognizer: PathBuf,
    pub liveness: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CameraConfig {
    #[serde(default)]
    pub rgb_device: u32,
    /// `None` when the host has no IR sensor; the installer probes
    /// V4L2 and writes whatever it finds.
    #[serde(default)]
    pub ir_device: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_threshold")]
    pub match_threshold: f32,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            match_threshold: default_threshold(),
        }
    }
}

const fn default_threshold() -> f32 {
    crate::verify::DEFAULT_MATCH_THRESHOLD
}

impl Config {
    /// Load `/etc/dax-auth/config.toml` if it exists.
    pub fn load_system() -> RuntimeResult<Option<Self>> {
        let path = Path::new(SYSTEM_CONFIG_PATH);
        if path.exists() {
            Self::load_from(path).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn load_from(path: &Path) -> RuntimeResult<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| RuntimeError::Config(format!("read {}: {e}", path.display())))?;
        let config: Self = toml::from_str(&raw)
            .map_err(|e| RuntimeError::Config(format!("parse {}: {e}", path.display())))?;
        Ok(config)
    }
}
