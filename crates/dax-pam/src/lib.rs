//! `dax-auth` PAM module.
//!
//! Linux PAM dispatches into a `cdylib` by looking up
//! `pam_sm_authenticate` (and friends) by symbol name. The
//! [`pam-bindings`] `pam_hooks!` macro wires that ABI for us, while
//! the actual face-recognition pipeline lives in `dax-runtime` so
//! the CLI and PAM share a single implementation.
//!
//! ## Threat model
//!
//! When `pam_authenticate` runs, the `.so` is `dlopen`ed inside the
//! caller's process (sudo, login, gdm, …). At that point the
//! caller's environment is **attacker-controlled**: a malicious
//! local user can set arbitrary `DAX_*` variables before invoking
//! `sudo`. We therefore deliberately ignore every environment
//! variable in this module and resolve the vault path, models,
//! camera index and passphrase **only** from root-owned files we
//! validate up front. The CLI keeps its environment overrides for
//! development workflows; the PAM module does not.

#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Required for the C ABI shim emitted by pam_hooks!.

use std::ffi::CStr;
use std::io::{IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use dax_runtime::{
    verify_face, Config, IrCheckOutcome, RuntimeError, VerifyConfig, VerifyReason,
    SYSTEM_CONFIG_PATH,
};
use pam::constants::{PamFlag, PamResultCode};
use pam::items::User;
use pam::module::{PamHandle, PamHooks};

/// Root-owned file holding the vault passphrase. Created at install
/// time with permissions 0600.
const SECRET_FILE: &str = "/etc/dax-auth/secret";

struct DaxPam;

pam::pam_hooks!(DaxPam);

impl PamHooks for DaxPam {
    fn sm_authenticate(pamh: &mut PamHandle, _args: Vec<&CStr>, _flags: PamFlag) -> PamResultCode {
        let user = match resolve_pam_user(pamh) {
            Ok(u) => u,
            Err(code) => return code,
        };
        let env = match load_environment() {
            Ok(env) => env,
            Err(reason) => {
                err(format_args!("config error: {reason}"));
                return PamResultCode::PAM_AUTH_ERR;
            }
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
            match_threshold: env.match_threshold,
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
    match_threshold: f32,
}

/// Load configuration from root-owned files. **Never** consults the
/// process environment: the caller of `sudo` controls those vars.
fn load_environment() -> Result<PamEnv, String> {
    let config_path = Path::new(SYSTEM_CONFIG_PATH);
    require_root_owned(config_path, 0o022)?;
    let config = Config::load_from(config_path).map_err(|e| format!("{e}"))?;

    let secret_path = Path::new(SECRET_FILE);
    // Anything more permissive than 0600 means the secret has leaked
    // to a non-root account; refuse to use it.
    require_root_owned(secret_path, 0o077)?;
    let passphrase = std::fs::read_to_string(secret_path)
        .map_err(|e| format!("read secret: {e}"))?
        .trim_end_matches('\n')
        .to_string();
    if passphrase.is_empty() {
        return Err(String::from("secret file is empty"));
    }

    Ok(PamEnv {
        vault: config.paths.vault,
        passphrase,
        detector: config.paths.detector,
        recognizer: config.paths.recognizer,
        liveness: config.paths.liveness,
        camera: config.camera.rgb_device,
        ir_camera: config.camera.ir_device,
        match_threshold: config.security.match_threshold,
    })
}

/// Validate that `path` is owned by root and that no bit in
/// `forbidden_mask` is set in the file mode. `forbidden_mask = 0o022`
/// rejects group/other write; `0o077` rejects any group/other access
/// at all (used for the secret).
fn require_root_owned(path: &Path, forbidden_mask: u32) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if meta.uid() != 0 {
        return Err(format!(
            "{} is not owned by root (uid={})",
            path.display(),
            meta.uid()
        ));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & forbidden_mask != 0 {
        return Err(format!(
            "{} has insecure permissions 0o{mode:o}",
            path.display()
        ));
    }
    Ok(())
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
