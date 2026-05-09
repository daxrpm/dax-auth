//! `dax-auth` PAM module.
//!
//! Linux PAM dispatches into a `cdylib` by looking up
//! `pam_sm_authenticate` (and friends) by symbol name. The
//! [`pam-bindings`] `pam_hooks!` macro wires that ABI for us, while
//! the actual face-recognition pipeline lives in `dax-runtime` so
//! the CLI and PAM share a single implementation.
//!
//! All filesystem paths and the vault passphrase are read from
//! environment variables to keep `PoC` integration simple — production
//! installs would hard-code them at build time or read a config in
//! `/etc/dax-auth/`.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Required for the C ABI shim emitted by pam_hooks!.

use std::ffi::CStr;
use std::path::PathBuf;

use dax_runtime::{verify_face, Config, VerifyConfig};
use pam::constants::{PamFlag, PamResultCode};
use pam::module::{PamHandle, PamHooks};
use tracing::{error, info, warn};

const ENV_VAULT: &str = "DAX_VAULT_PATH";
const ENV_PASSPHRASE: &str = "DAX_VAULT_PASSPHRASE";
const ENV_DETECTOR: &str = "DAX_DETECTOR_MODEL";
const ENV_RECOGNIZER: &str = "DAX_RECOGNIZER_MODEL";
const ENV_LIVENESS: &str = "DAX_LIVENESS_MODEL";
const ENV_CAMERA: &str = "DAX_CAMERA_DEVICE";

/// Root-owned file holding the vault passphrase. Created at install
/// time. PAM `.so` files inherit a sanitised env from the parent
/// process, so reading the passphrase from a 600-perm file under
/// `/etc/dax-auth/` is the only reliable way to ship it.
const SECRET_FILE: &str = "/etc/dax-auth/secret";

struct DaxPam;

pam::pam_hooks!(DaxPam);

impl PamHooks for DaxPam {
    fn sm_authenticate(pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        // Loud breadcrumbs to stderr — these always reach the TTY of
        // whatever loaded us (sudo, login, pamtester) so we can tell
        // from the user's terminal whether our hook ran at all.
        eprintln!("[dax-pam] sm_authenticate entered");
        init_logging();

        let user = match pamh.get_user(None) {
            Ok(u) => {
                eprintln!("[dax-pam] target user = {u}");
                u
            }
            Err(e) => {
                eprintln!("[dax-pam] get_user failed: {e:?}");
                return e;
            }
        };
        let env = match read_env() {
            Ok(env) => env,
            Err(missing) => {
                eprintln!("[dax-pam] config error: {missing}");
                error!("dax-pam: missing required configuration: {missing}");
                return PamResultCode::PAM_AUTH_ERR;
            }
        };
        eprintln!(
            "[dax-pam] config ok, vault={} detector={}",
            env.vault.display(),
            env.detector.display()
        );

        let config = VerifyConfig {
            user: &user,
            vault_path: &env.vault,
            passphrase: env.passphrase.as_bytes(),
            camera_index: env.camera,
            detector_path: &env.detector,
            recognizer_path: &env.recognizer,
            liveness_path: &env.liveness,
            match_threshold: dax_runtime::DEFAULT_MATCH_THRESHOLD,
        };

        eprintln!("[dax-pam] running verify_face …");
        match verify_face(&config) {
            Ok(outcome) if outcome.matched => {
                eprintln!(
                    "[dax-pam] MATCH cosine={:.4} real={:.4}",
                    outcome.best_cosine, outcome.liveness_real
                );
                info!(
                    user = %user,
                    cosine = outcome.best_cosine,
                    real = outcome.liveness_real,
                    "dax-pam: authenticated"
                );
                PamResultCode::PAM_SUCCESS
            }
            Ok(outcome) => {
                eprintln!(
                    "[dax-pam] REJECT reason={:?} cosine={:.4} real={:.4} spoof={:.4}",
                    outcome.reason,
                    outcome.best_cosine,
                    outcome.liveness_real,
                    outcome.liveness_spoof
                );
                warn!(
                    user = %user,
                    cosine = outcome.best_cosine,
                    real = outcome.liveness_real,
                    spoof = outcome.liveness_spoof,
                    reason = ?outcome.reason,
                    "dax-pam: rejected"
                );
                PamResultCode::PAM_AUTH_ERR
            }
            Err(err) => {
                eprintln!("[dax-pam] pipeline error: {err}");
                error!(user = %user, error = %err, "dax-pam: pipeline error");
                PamResultCode::PAM_AUTH_ERR
            }
        }
    }

    fn sm_setcred(_pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        // We do not manage credentials beyond the authentication
        // decision itself; PAM still expects this hook to exist.
        PamResultCode::PAM_SUCCESS
    }

    fn acct_mgmt(_pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        PamResultCode::PAM_SUCCESS
    }
}

struct PamEnv {
    vault: PathBuf,
    passphrase: String,
    detector: PathBuf,
    recognizer: PathBuf,
    liveness: PathBuf,
    camera: u32,
}

fn read_env() -> Result<PamEnv, String> {
    // 1. Prefer environment variables when present (developer flow,
    //    pamtester runs).
    let env_vault = std::env::var(ENV_VAULT).ok();
    let env_passphrase = std::env::var(ENV_PASSPHRASE).ok();
    let env_detector = std::env::var(ENV_DETECTOR).ok();
    let env_recognizer = std::env::var(ENV_RECOGNIZER).ok();
    let env_liveness = std::env::var(ENV_LIVENESS).ok();
    let env_camera = std::env::var(ENV_CAMERA)
        .ok()
        .and_then(|s| s.parse::<u32>().ok());

    // 2. Fall back to the system config so an installed system
    //    works without callers exporting DAX_* vars.
    let config = Config::load_system().ok().flatten();
    let secret = std::fs::read_to_string(SECRET_FILE)
        .ok()
        .map(|s| s.trim_end_matches('\n').to_string());

    let vault = env_vault
        .map(PathBuf::from)
        .or_else(|| config.as_ref().map(|c| c.paths.vault.clone()))
        .ok_or_else(|| format!("vault path ({ENV_VAULT} or config.toml)"))?;
    let detector = env_detector
        .map(PathBuf::from)
        .or_else(|| config.as_ref().map(|c| c.paths.detector.clone()))
        .ok_or_else(|| format!("detector model ({ENV_DETECTOR} or config.toml)"))?;
    let recognizer = env_recognizer
        .map(PathBuf::from)
        .or_else(|| config.as_ref().map(|c| c.paths.recognizer.clone()))
        .ok_or_else(|| format!("recognizer model ({ENV_RECOGNIZER} or config.toml)"))?;
    let liveness = env_liveness
        .map(PathBuf::from)
        .or_else(|| config.as_ref().map(|c| c.paths.liveness.clone()))
        .ok_or_else(|| format!("liveness model ({ENV_LIVENESS} or config.toml)"))?;
    let camera = env_camera
        .or_else(|| config.as_ref().map(|c| c.camera.rgb_device))
        .unwrap_or(0);
    let passphrase = env_passphrase
        .or(secret)
        .ok_or_else(|| format!("vault passphrase ({ENV_PASSPHRASE} or {SECRET_FILE})"))?;

    Ok(PamEnv {
        vault,
        passphrase,
        detector,
        recognizer,
        liveness,
        camera,
    })
}

fn init_logging() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact()
            .try_init();
    });
}
