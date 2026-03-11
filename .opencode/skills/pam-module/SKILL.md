# Skill: PAM Module — dax-auth-pam

## When to use
Load this skill when working on `crates/dax-auth-pam/` (the `pam_dax_auth.so` cdylib).

---

## Critical constraints

The PAM module is a C dynamic library called by `libpam`. It has HARD constraints:

| Constraint | Reason |
|---|---|
| **NO tokio / NO async** | PAM callbacks are synchronous C ABI — no async runtime allowed |
| **NO heavy deps** | Must link fast — adds latency to every login/sudo |
| **NO ML code** | Never link dax-auth-core — talk to daemon via socket |
| **`#[no_mangle]` on exports** | C ABI requires exact symbol names |
| **`extern "C"` on exports** | C calling convention |
| **`cdylib` crate type** | Produces `.so` file (not `.rlib`) |

---

## Required PAM exports

These four functions MUST exist with exact names:

```rust
/// Called for the auth phase (verify user's face).
#[no_mangle]
pub extern "C" fn pam_sm_authenticate(
    pamh: *mut pam_sys::pam_handle_t,
    _flags: libc::c_int,
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> libc::c_int { ... }

/// Called for account management (check if account is valid).
#[no_mangle]
pub extern "C" fn pam_sm_acct_mgmt(
    _pamh: *mut pam_sys::pam_handle_t,
    _flags: libc::c_int,
    _argc: libc::c_int,
    _argv: *const *const libc::c_char,
) -> libc::c_int {
    pam_sys::PAM_SUCCESS  // not our concern
}

/// Called for session open.
#[no_mangle]
pub extern "C" fn pam_sm_open_session(...) -> libc::c_int { pam_sys::PAM_SUCCESS }

/// Called for session close.
#[no_mangle]
pub extern "C" fn pam_sm_close_session(...) -> libc::c_int { pam_sys::PAM_SUCCESS }
```

---

## PAM return codes

```rust
use pam_sys::{PAM_SUCCESS, PAM_AUTH_ERR, PAM_USER_UNKNOWN, PAM_SYSTEM_ERR, PAM_IGNORE};

// PAM_SUCCESS    = 0  → authentication granted
// PAM_AUTH_ERR   = 7  → authentication failed (face not recognized)
// PAM_USER_UNKNOWN = 10 → username not found / no enrolled faces
// PAM_SYSTEM_ERR = 4  → daemon not running / socket error
// PAM_IGNORE     = 25 → skip this module (use when daemon unavailable as fallback)
```

---

## Getting the PAM username

```rust
unsafe fn get_pam_user(pamh: *mut pam_sys::pam_handle_t) -> Option<String> {
    let mut user_ptr: *const libc::c_char = std::ptr::null();
    let ret = pam_sys::pam_get_user(pamh, &mut user_ptr, std::ptr::null());
    if ret != pam_sys::PAM_SUCCESS || user_ptr.is_null() {
        return None;
    }
    // SAFETY: pam_get_user guarantees a valid, null-terminated C string
    //         for the lifetime of the PAM transaction.
    let cstr = std::ffi::CStr::from_ptr(user_ptr);
    cstr.to_str().ok().map(String::from)
}
```

---

## Socket communication (synchronous only)

```rust
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const SOCKET_PATH: &str = "/run/dax-auth/daemon.sock";
const TIMEOUT_SECS: u64 = 30;  // facial auth timeout

fn authenticate_via_daemon(user: &str, security_mode: SecurityMode) -> Result<AuthResult, ()> {
    let mut stream = UnixStream::connect(SOCKET_PATH).map_err(|_| ())?;
    stream.set_read_timeout(Some(Duration::from_secs(TIMEOUT_SECS))).map_err(|_| ())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|_| ())?;

    let user_id = UserId::new(user).map_err(|_| ())?;
    let request = AuthRequest::new(user_id, security_mode);
    let encoded = dax_auth_proto::codec::encode(&request).map_err(|_| ())?;

    stream.write_all(&encoded).map_err(|_| ())?;

    let mut len_buf = [0u8; 8];  // version (4) + length (4)
    stream.read_exact(&mut len_buf).map_err(|_| ())?;
    let length = u32::from_le_bytes(len_buf[4..8].try_into().unwrap()) as usize;

    let mut payload = vec![0u8; 8 + length];
    payload[..8].copy_from_slice(&len_buf);
    stream.read_exact(&mut payload[8..]).map_err(|_| ())?;

    let response: AuthResponse = dax_auth_proto::codec::decode(&payload).map_err(|_| ())?;
    Ok(response.result)
}
```

---

## Fallback behavior

If the daemon is unreachable → return `PAM_IGNORE` (NOT `PAM_AUTH_ERR`).
This allows `libpam` to continue to the next module (typically password auth).
NEVER block login if the daemon is down.

```rust
match authenticate_via_daemon(&username, mode) {
    Ok(AuthResult::Granted { .. }) => pam_sys::PAM_SUCCESS,
    Ok(AuthResult::Denied(_))      => pam_sys::PAM_AUTH_ERR,
    Err(_)                         => pam_sys::PAM_IGNORE,  // daemon down → try password
}
```

---

## Build artifact

The built `.so` must be copied to `/usr/lib/security/pam_dax_auth.so`.
PAM config snippet:
```
# /etc/pam.d/sudo (or common-auth)
auth  sufficient  pam_dax_auth.so
```
