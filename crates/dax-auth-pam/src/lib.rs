//! # pam_dax_auth
//!
//! PAM module for dax-auth facial authentication.
//!
//! ## Design constraints
//! - This is a `cdylib` — loaded into the PAM caller's process space
//! - MUST be minimal: no tokio, no async, no heavy allocations
//! - Uses blocking I/O to communicate with `dax-authd` via Unix socket
//! - Must handle daemon-not-running gracefully (fall through to next PAM module)
//! - All PAM-required C symbols are exported via `#[no_mangle]`
//!
//! ## PAM conversation
//! The module does NOT ask for a password. It:
//! 1. Gets the username from PAM
//! 2. Connects to `/run/dax-auth/daemon.sock`
//! 3. Sends `AuthRequest` (bincode framed)
//! 4. Waits for `AuthResponse` (with timeout)
//! 5. Returns `PAM_SUCCESS` or `PAM_AUTH_ERR`

// We need unsafe here for the PAM C ABI — carefully contained
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use dax_auth_proto::{codec, AuthRequest, AuthResponse, SecurityMode, UserId, SOCKET_PATH};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// PAM return codes (from pam_types.h)
const PAM_SUCCESS: i32 = 0;
const PAM_AUTH_ERR: i32 = 7;
const PAM_IGNORE: i32 = 25;
const PAM_SERVICE_ERR: i32 = 3;

/// Entry point called by PAM for authentication.
///
/// # Safety
/// This function is called by the PAM framework via C ABI.
/// `pamh` is guaranteed valid by the framework when this is called.
#[no_mangle]
pub extern "C" fn pam_sm_authenticate(
    pamh: *mut libc::c_void,
    _flags: i32,
    _argc: i32,
    _argv: *const *const libc::c_char,
) -> i32 {
    match authenticate_inner(pamh) {
        Ok(true) => PAM_SUCCESS,
        Ok(false) => PAM_AUTH_ERR,
        Err(e) => {
            // Log to syslog — TODO: implement
            let _ = e;
            // If daemon is unavailable, ignore (fall through to password)
            PAM_IGNORE
        }
    }
}

/// Entry point for session open — we don't use this but PAM requires it.
#[no_mangle]
pub extern "C" fn pam_sm_open_session(
    _pamh: *mut libc::c_void,
    _flags: i32,
    _argc: i32,
    _argv: *const *const libc::c_char,
) -> i32 {
    PAM_SUCCESS
}

/// Entry point for session close.
#[no_mangle]
pub extern "C" fn pam_sm_close_session(
    _pamh: *mut libc::c_void,
    _flags: i32,
    _argc: i32,
    _argv: *const *const libc::c_char,
) -> i32 {
    PAM_SUCCESS
}

/// Inner authentication logic.
///
/// Returns `Ok(true)` on grant, `Ok(false)` on deny, `Err` on system error.
fn authenticate_inner(pamh: *mut libc::c_void) -> Result<bool, PamModuleError> {
    // TODO Phase 1:
    // 1. Get username from PAM handle
    // 2. Read user's security mode from /etc/dax-auth/config.toml (or per-user config)
    // 3. Connect to Unix socket with short timeout
    // 4. Send AuthRequest
    // 5. Read AuthResponse
    // 6. Return result
    todo!("implement PAM authentication")
}

/// Errors within the PAM module.
#[derive(Debug)]
enum PamModuleError {
    /// Cannot get username from PAM.
    NoUsername,
    /// Daemon socket not available.
    DaemonUnavailable,
    /// Protocol error.
    Protocol(String),
    /// I/O error.
    Io(std::io::Error),
}

impl From<std::io::Error> for PamModuleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// Link against libc for PAM types
extern crate libc;
