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

use dax_runtime::{verify_face, VerifyConfig, VerifyReason};
use pam::constants::{PamFlag, PamResultCode};
use pam::module::{PamHandle, PamHooks};
use pam::pam_try;
use tracing::{error, info, warn};

const ENV_VAULT: &str = "DAX_VAULT_PATH";
const ENV_PASSPHRASE: &str = "DAX_VAULT_PASSPHRASE";
const ENV_DETECTOR: &str = "DAX_DETECTOR_MODEL";
const ENV_RECOGNIZER: &str = "DAX_RECOGNIZER_MODEL";
const ENV_LIVENESS: &str = "DAX_LIVENESS_MODEL";
const ENV_CAMERA: &str = "DAX_CAMERA_DEVICE";

struct DaxPam;

pam::pam_hooks!(DaxPam);

impl PamHooks for DaxPam {
    fn sm_authenticate(pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        init_logging();

        let user = pam_try!(pamh.get_user(None));
        let env = match read_env() {
            Ok(env) => env,
            Err(missing) => {
                error!("dax-pam: missing required env var `{missing}`");
                return PamResultCode::PAM_AUTH_ERR;
            }
        };

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

        match verify_face(&config) {
            Ok(outcome) if outcome.matched => {
                info!(
                    user = %user,
                    cosine = outcome.best_cosine,
                    real = outcome.liveness_real,
                    "dax-pam: authenticated"
                );
                PamResultCode::PAM_SUCCESS
            }
            Ok(outcome) => {
                warn!(
                    user = %user,
                    cosine = outcome.best_cosine,
                    real = outcome.liveness_real,
                    spoof = outcome.liveness_spoof,
                    reason = ?outcome.reason,
                    "dax-pam: rejected"
                );
                match outcome.reason {
                    VerifyReason::LivenessSpoof => PamResultCode::PAM_AUTH_ERR,
                    VerifyReason::BelowThreshold | VerifyReason::Match => {
                        PamResultCode::PAM_AUTH_ERR
                    }
                }
            }
            Err(err) => {
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

fn read_env() -> Result<PamEnv, &'static str> {
    let vault = std::env::var(ENV_VAULT).map_err(|_| ENV_VAULT)?.into();
    let passphrase = std::env::var(ENV_PASSPHRASE).map_err(|_| ENV_PASSPHRASE)?;
    let detector = std::env::var(ENV_DETECTOR)
        .map_err(|_| ENV_DETECTOR)?
        .into();
    let recognizer = std::env::var(ENV_RECOGNIZER)
        .map_err(|_| ENV_RECOGNIZER)?
        .into();
    let liveness = std::env::var(ENV_LIVENESS)
        .map_err(|_| ENV_LIVENESS)?
        .into();
    let camera = std::env::var(ENV_CAMERA)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
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
