//! `dax-auth` PAM module.
//!
//! Linux PAM dispatches into a `cdylib` by looking up
//! `pam_sm_authenticate` (and friends) by symbol name. The
//! [`pam-bindings`] `pam_hooks!` macro wires that ABI for us, while
//! the actual face-recognition pipeline lives in `dax-runtime` so
//! the CLI and PAM share a single implementation.
//!
//! Output policy: a single status line on the calling TTY, coloured
//! when the destination is interactive. Successful auths print
//! something like `✓  authenticated  ·  sim 70%  ·  live 99%`;
//! rejections print a one-liner explaining why and let the calling
//! stack (sudo, login, …) fall through to its password prompt.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Required for the C ABI shim emitted by pam_hooks!.

use std::ffi::CStr;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use dax_runtime::{verify_face, Config, IrCheckOutcome, RuntimeError, VerifyConfig, VerifyReason};
use pam::constants::{PamFlag, PamResultCode};
use pam::items::User;
use pam::module::{PamHandle, PamHooks};

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
        let user = match resolve_pam_user(pamh) {
            Ok(u) => u,
            Err(code) => return code,
        };
        let Ok(env) = read_env() else {
            return PamResultCode::PAM_AUTH_ERR;
        };

        let config = VerifyConfig {
            user: &user,
            vault_path: &env.vault,
            passphrase: env.passphrase.as_bytes(),
            camera_index: env.camera,
            ir_camera_index: env.ir_camera,
            detector_path: &env.detector,
            recognizer_path: &env.recognizer,
            liveness_path: &env.liveness,
            match_threshold: dax_runtime::DEFAULT_MATCH_THRESHOLD,
            ir_center_tolerance: dax_runtime::DEFAULT_IR_CENTER_TOLERANCE,
        };

        match verify_face(&config) {
            Ok(outcome) if outcome.matched => {
                let ir = ir_label(outcome.ir_check);
                ok(format_args!(
                    "authenticated  ·  sim {sim:.0}%  ·  live {live:.0}%{ir}",
                    sim = outcome.best_cosine * 100.0,
                    live = outcome.liveness_real * 100.0,
                ));
                PamResultCode::PAM_SUCCESS
            }
            Ok(outcome) => {
                match outcome.reason {
                    VerifyReason::LivenessSpoof => warn(format_args!(
                        "spoof detected  ·  live {live:.0}%  ·  spoof {sp:.0}%",
                        live = outcome.liveness_real * 100.0,
                        sp = outcome.liveness_spoof * 100.0,
                    )),
                    VerifyReason::IrCrossCheckFailed => warn(format_args!(
                        "spoof detected  ·  ir {what}",
                        what = match outcome.ir_check {
                            IrCheckOutcome::NoFace => "saw no face",
                            IrCheckOutcome::OffPosition => "bbox mismatch",
                            _ => "rejected",
                        }
                    )),
                    VerifyReason::BelowThreshold => warn(format_args!(
                        "no match  ·  sim {sim:.0}%  ·  threshold {thr:.0}%",
                        sim = outcome.best_cosine * 100.0,
                        thr = config.match_threshold * 100.0,
                    )),
                    VerifyReason::Match => warn(format_args!("unexpected match path")),
                }
                PamResultCode::PAM_AUTH_ERR
            }
            Err(RuntimeError::NoFace) => {
                err(format_args!("no face detected"));
                PamResultCode::PAM_AUTH_ERR
            }
            Err(RuntimeError::UserNotEnrolled(u)) => {
                err(format_args!("user `{u}` is not enrolled"));
                PamResultCode::PAM_AUTH_ERR
            }
            Err(other) => {
                err(format_args!("{other}"));
                PamResultCode::PAM_AUTH_ERR
            }
        }
    }

    fn sm_setcred(_pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
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
    ir_camera: Option<u32>,
}

fn read_env() -> Result<PamEnv, String> {
    let env_vault = std::env::var(ENV_VAULT).ok();
    let env_passphrase = std::env::var(ENV_PASSPHRASE).ok();
    let env_detector = std::env::var(ENV_DETECTOR).ok();
    let env_recognizer = std::env::var(ENV_RECOGNIZER).ok();
    let env_liveness = std::env::var(ENV_LIVENESS).ok();
    let env_camera = std::env::var(ENV_CAMERA)
        .ok()
        .and_then(|s| s.parse::<u32>().ok());

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
    let ir_camera = config.as_ref().and_then(|c| c.camera.ir_device);
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
        ir_camera,
    })
}

/// Render the IR cross-check result as a status-line suffix.
fn ir_label(check: IrCheckOutcome) -> &'static str {
    match check {
        IrCheckOutcome::Disabled => "",
        IrCheckOutcome::Matched => "  ·  ir ok",
        IrCheckOutcome::NoFace => "  ·  ir no face",
        IrCheckOutcome::OffPosition => "  ·  ir mismatch",
    }
}

// ──────────────────────── status output ────────────────────────
//
// Tiny, dependency-free formatter. ANSI colour escapes are emitted
// only when stderr is a TTY; piped output stays clean.

const C_GREEN: &str = "\x1b[1;32m";
const C_YELLOW: &str = "\x1b[1;33m";
const C_RED: &str = "\x1b[1;31m";
const C_DIM: &str = "\x1b[2m";
const C_RESET: &str = "\x1b[0m";

fn ok(args: std::fmt::Arguments<'_>) {
    write_status('\u{2713}', C_GREEN, args);
}

fn warn(args: std::fmt::Arguments<'_>) {
    write_status('\u{2717}', C_YELLOW, args);
}

fn err(args: std::fmt::Arguments<'_>) {
    write_status('\u{2717}', C_RED, args);
}

fn write_status(symbol: char, color: &str, args: std::fmt::Arguments<'_>) {
    let mut stderr = std::io::stderr();
    let coloured = stderr.is_terminal();
    let _ = if coloured {
        writeln!(
            stderr,
            "{color}{symbol}{C_RESET}  {C_DIM}dax-auth{C_RESET}  {args}"
        )
    } else {
        writeln!(stderr, "{symbol}  dax-auth  {args}")
    };
}

/// Read the PAM user, working around `pam-bindings 0.1.1`'s
/// `get_user` which returns `Err(PAM_SUCCESS)` on the happy path
/// because of a `*const *mut`/`*mut *const` mismatch in the FFI
/// glue. `get_item::<User>()` uses `&mut ptr` correctly, so the
/// username comes through.
fn resolve_pam_user(pamh: &mut PamHandle) -> Result<String, PamResultCode> {
    if let Ok(u) = pamh.get_user(None) {
        if !u.is_empty() {
            return Ok(u);
        }
    }
    match pamh.get_item::<User<'_>>() {
        Ok(Some(item)) => item
            .0
            .to_str()
            .map(str::to_owned)
            .map_err(|_| PamResultCode::PAM_AUTH_ERR),
        Ok(None) => Err(PamResultCode::PAM_USER_UNKNOWN),
        Err(PamResultCode::PAM_SUCCESS) => Err(PamResultCode::PAM_AUTH_ERR),
        Err(other) => Err(other),
    }
}
