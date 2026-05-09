//! Resolve effective configuration for a CLI invocation.
//!
//! Sources, in priority order:
//!   1. Explicit `--flag` overrides passed by the caller.
//!   2. The system config at `/etc/dax-auth/config.toml`.
//!
//! For the vault passphrase the order is:
//!   1. `DAX_VAULT_PASSPHRASE` environment variable (developer flow).
//!   2. `/etc/dax-auth/secret` (root-readable, written by the
//!      installer).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dax_runtime::{Config, SYSTEM_CONFIG_PATH};

const ENV_PASSPHRASE: &str = "DAX_VAULT_PASSPHRASE";
const SECRET_PATH: &str = "/etc/dax-auth/secret";

#[derive(Debug, Clone)]
pub struct Resolved {
    pub vault: PathBuf,
    pub passphrase: String,
    pub detector: PathBuf,
    pub recognizer: PathBuf,
    pub liveness: PathBuf,
    pub camera_index: u32,
}

#[derive(Debug, Default)]
pub struct Overrides<'a> {
    pub vault: Option<&'a Path>,
    pub detector: Option<&'a Path>,
    pub recognizer: Option<&'a Path>,
    pub liveness: Option<&'a Path>,
    pub camera_index: Option<u32>,
}

#[allow(clippy::needless_pass_by_value)]
pub fn resolve(overrides: Overrides<'_>) -> Result<Resolved> {
    let config: Option<Config> =
        Config::load_system().map_err(|e| anyhow::anyhow!("loading {SYSTEM_CONFIG_PATH}: {e}"))?;

    let vault = pick(
        overrides.vault,
        config.as_ref().map(|c| c.paths.vault.as_path()),
    )
    .with_context(|| missing("vault path", "--vault"))?;
    let detector = pick(
        overrides.detector,
        config.as_ref().map(|c| c.paths.detector.as_path()),
    )
    .with_context(|| missing("detector model", "--detector"))?;
    let recognizer = pick(
        overrides.recognizer,
        config.as_ref().map(|c| c.paths.recognizer.as_path()),
    )
    .with_context(|| missing("recognizer model", "--recognizer"))?;
    let liveness = pick(
        overrides.liveness,
        config.as_ref().map(|c| c.paths.liveness.as_path()),
    )
    .with_context(|| missing("liveness model", "--liveness-model"))?;
    let camera_index = overrides
        .camera_index
        .or_else(|| config.as_ref().map(|c| c.camera.rgb_device))
        .unwrap_or(0);

    let passphrase = read_passphrase()?;

    Ok(Resolved {
        vault,
        passphrase,
        detector,
        recognizer,
        liveness,
        camera_index,
    })
}

fn pick(arg: Option<&Path>, cfg: Option<&Path>) -> Option<PathBuf> {
    arg.map(PathBuf::from).or_else(|| cfg.map(PathBuf::from))
}

fn read_passphrase() -> Result<String> {
    if let Ok(p) = std::env::var(ENV_PASSPHRASE) {
        return Ok(p);
    }
    match std::fs::read_to_string(SECRET_PATH) {
        Ok(s) => Ok(s.trim_end_matches('\n').to_string()),
        Err(e) => Err(anyhow::anyhow!(
            "vault passphrase not available: set {ENV_PASSPHRASE} or run as root so {SECRET_PATH} \
             can be read ({e})"
        )),
    }
}

fn missing(what: &str, flag: &str) -> String {
    format!(
        "{what} not configured: pass {flag} or run sudo ./scripts/install.sh \
         to create /etc/dax-auth/config.toml"
    )
}

/// Verify the calling process can create / overwrite a file at
/// `path`. Catches the common "ran without sudo" failure mode in
/// milliseconds, before we spend seconds loading ONNX models.
pub fn ensure_writable(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        anyhow::bail!(
            "vault directory does not exist: {}\n  Run sudo ./scripts/install.sh first.",
            parent.display()
        );
    }
    let probe = parent.join(format!(".dax-auth-probe.{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(anyhow::anyhow!(
            "no write permission on {} — re-run with `sudo` (or pass --vault to a path you can write).",
            parent.display()
        )),
        Err(e) => Err(anyhow::anyhow!(
            "write probe on {} failed: {e}",
            parent.display()
        )),
    }
}

/// Pick the user this command should target.
///
/// Heuristic: when invoked under `sudo` we want the human user, not
/// `root`, so `$SUDO_USER` wins. Otherwise fall back to `$USER`.
/// Callers can always pass `--user` to override.
pub fn default_user() -> Result<String> {
    if let Ok(u) = std::env::var("SUDO_USER") {
        if !u.is_empty() && u != "root" {
            return Ok(u);
        }
    }
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return Ok(u);
        }
    }
    anyhow::bail!("could not determine target user; pass --user explicitly")
}
