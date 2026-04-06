//! # pam_dax_auth
//!
//! PAM module for dax-auth facial authentication.
//!
//! ## Design constraints
//! - This is a `cdylib` — loaded into the PAM caller's process space
//! - MUST be minimal: no tokio, no async, no heavy allocations
//! - Uses blocking I/O to communicate with `dax-authd` via Unix socket
//! - Daemon/I/O failures are fail-closed by default (`PAM_AUTH_ERR`)
//! - Optional compatibility mode `fail_open=1` allows PAM fallthrough (`PAM_IGNORE`)
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
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

// ------------------------------------------------------------------
// PAM return codes (from /usr/include/security/_pam_types.h)
// ------------------------------------------------------------------

/// Authentication succeeded.
const PAM_SUCCESS: i32 = 0;

/// Service error (internal failure in the module).
const PAM_SERVICE_ERR: i32 = 3;

/// Authentication failure (face not recognised, liveness failed, etc.).
const PAM_AUTH_ERR: i32 = 7;

/// Module should be ignored (daemon unavailable, no camera, etc.).
///
/// PAM will continue to the next module in the stack.
const PAM_IGNORE: i32 = 25;

// ------------------------------------------------------------------
// pam_get_item constants
// ------------------------------------------------------------------

/// `PAM_SERVICE` — the service name (e.g., "sudo", "login").
#[allow(dead_code)]
const PAM_SERVICE: i32 = 1;

/// `PAM_USER` — the username being authenticated.
const PAM_USER: i32 = 2;

// ------------------------------------------------------------------
// Timeouts
// ------------------------------------------------------------------

/// How long the PAM module waits for the daemon to respond.
/// The daemon itself has an internal pipeline timeout (typically 8s).
const DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_READ_TIMEOUT: Duration = Duration::from_secs(12);
const DAEMON_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

// ------------------------------------------------------------------
// C ABI entry points (exported symbols)
// ------------------------------------------------------------------

/// Entry point called by PAM for authentication.
///
/// # Safety
/// This function is called by the PAM framework via C ABI.
/// `pamh` is guaranteed valid by the framework when this is called.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut libc::c_void,
    _flags: i32,
    argc: i32,
    argv: *const *const libc::c_char,
) -> i32 {
    let options = parse_module_options(argc, argv);

    match authenticate_inner(pamh) {
        Ok(true) => PAM_SUCCESS,
        Ok(false) => PAM_AUTH_ERR,
        Err(PamModuleError::NoUsername) => {
            log_syslog(libc::LOG_ERR, "dax-auth: could not get username from PAM");
            PAM_AUTH_ERR
        }
        Err(PamModuleError::DaemonUnavailable) => {
            if options.fail_open {
                log_syslog(
                    libc::LOG_WARNING,
                    "dax-auth: daemon unavailable, fail_open enabled, falling through",
                );
                PAM_IGNORE
            } else {
                log_syslog(
                    libc::LOG_ERR,
                    "dax-auth: daemon unavailable, fail-closed denying authentication",
                );
                PAM_AUTH_ERR
            }
        }
        Err(PamModuleError::Protocol(ref msg)) => {
            let s = format!("dax-auth: protocol error: {msg}");
            log_syslog(libc::LOG_ERR, &s);
            PAM_SERVICE_ERR
        }
        Err(PamModuleError::Io(ref e)) => {
            let s = format!("dax-auth: I/O error: {e}");
            log_syslog(libc::LOG_ERR, &s);
            if options.fail_open {
                PAM_IGNORE
            } else {
                PAM_AUTH_ERR
            }
        }
    }
}

/// Entry point for session open — we don't need this but PAM may require it.
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

/// Entry point for setting credentials (required by some PAM stacks).
#[no_mangle]
pub extern "C" fn pam_sm_setcred(
    _pamh: *mut libc::c_void,
    _flags: i32,
    _argc: i32,
    _argv: *const *const libc::c_char,
) -> i32 {
    PAM_SUCCESS
}

// ------------------------------------------------------------------
// Core authentication logic
// ------------------------------------------------------------------

/// Inner authentication logic — all unsafe is contained here and in `get_pam_user`.
///
/// Returns:
/// - `Ok(true)` — face recognised, grant access
/// - `Ok(false)` — face present but not recognised / liveness failed
/// - `Err(DaemonUnavailable)` — daemon not running (PAM should ignore us)
/// - `Err(_)` — other system error
fn authenticate_inner(pamh: *mut libc::c_void) -> Result<bool, PamModuleError> {
    // 1. Get username from the PAM handle
    let username = unsafe { get_pam_user(pamh) }?;

    // 2. Build the UserId (validates length etc.)
    let user_id = UserId::new(&username).map_err(|e| PamModuleError::Protocol(e.to_string()))?;

    // 3. Build the request — default SecurityMode::Secure.
    //    In a future version we could read /etc/dax-auth/config.toml here,
    //    but we keep the PAM module minimal and let the daemon apply the policy.
    let request = AuthRequest::new(user_id, SecurityMode::Secure);

    // 4. Encode the request frame
    let frame = codec::encode(&request).map_err(|e| PamModuleError::Protocol(e.to_string()))?;

    // 5. Connect to daemon socket (non-blocking connect with timeout via UnixStream)
    let mut stream = connect_with_timeout(SOCKET_PATH, DAEMON_CONNECT_TIMEOUT).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound
            || e.kind() == io::ErrorKind::ConnectionRefused
            || e.kind() == io::ErrorKind::TimedOut
        {
            PamModuleError::DaemonUnavailable
        } else {
            PamModuleError::Io(e)
        }
    })?;

    // 6. Set read/write timeouts so we never block the PAM caller indefinitely
    stream
        .set_write_timeout(Some(DAEMON_WRITE_TIMEOUT))
        .map_err(PamModuleError::Io)?;
    stream
        .set_read_timeout(Some(DAEMON_READ_TIMEOUT))
        .map_err(PamModuleError::Io)?;

    // 7. Send the request frame
    stream.write_all(&frame).map_err(|e| {
        if e.kind() == io::ErrorKind::BrokenPipe || e.kind() == io::ErrorKind::ConnectionReset {
            PamModuleError::DaemonUnavailable
        } else {
            PamModuleError::Io(e)
        }
    })?;

    // 8. Read the response frame (version u32 + length u32 + payload)
    let response = read_response(&mut stream)?;

    // 9. Interpret the result
    Ok(response.is_granted())
}

// ------------------------------------------------------------------
// Helper: connect with a timeout
//
// `std::os::unix::net::UnixStream` has no built-in connect timeout,
// so we use `nix` to do a non-blocking connect + `select`.
// ------------------------------------------------------------------

/// Connect to a Unix socket with a wall-clock timeout.
///
/// Returns the connected (and restored to blocking) `UnixStream`, or an `io::Error`.
fn connect_with_timeout(path: &str, timeout: Duration) -> io::Result<UnixStream> {
    use std::os::unix::io::FromRawFd;

    // Create a non-blocking socket via libc
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    // Build sockaddr_un
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;

    let path_bytes = path.as_bytes();
    if path_bytes.len() >= addr.sun_path.len() {
        unsafe { libc::close(fd) };
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    // SAFETY: sun_path is an array of i8 on Linux; we copy ASCII bytes
    for (i, &b) in path_bytes.iter().enumerate() {
        addr.sun_path[i] = b as libc::c_char;
    }

    let addr_len = (std::mem::size_of::<libc::sa_family_t>() + path_bytes.len() + 1/* NUL */)
        as libc::socklen_t;

    // Attempt non-blocking connect
    let ret = unsafe {
        libc::connect(
            fd,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            addr_len,
        )
    };

    let connect_err = if ret < 0 {
        io::Error::last_os_error()
    } else {
        // Connected immediately (rare for Unix sockets but possible)
        // Restore blocking mode and return
        unsafe { set_blocking(fd)? };
        return Ok(unsafe { UnixStream::from_raw_fd(fd) });
    };

    // EINPROGRESS means the connect is in flight — wait via select(2)
    if connect_err.raw_os_error() != Some(libc::EINPROGRESS) {
        unsafe { libc::close(fd) };
        return Err(connect_err);
    }

    let fd_setsize = libc::FD_SETSIZE as libc::c_int;
    if fd < 0 || fd >= fd_setsize {
        unsafe { libc::close(fd) };
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket fd out of range for FD_SET: {fd} >= FD_SETSIZE={fd_setsize}"),
        ));
    }

    // select() with timeout
    let mut write_fds: libc::fd_set = unsafe { std::mem::zeroed() };
    unsafe { libc::FD_SET(fd, &mut write_fds) };

    let secs = timeout.as_secs() as libc::time_t;
    let usecs = timeout.subsec_micros() as libc::suseconds_t;
    let mut tv = libc::timeval {
        tv_sec: secs,
        tv_usec: usecs,
    };

    let nfds = fd + 1;
    let ready = unsafe {
        libc::select(
            nfds,
            std::ptr::null_mut(),
            &mut write_fds,
            std::ptr::null_mut(),
            &mut tv,
        )
    };

    if ready < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }

    if ready == 0 {
        unsafe { libc::close(fd) };
        return Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out"));
    }

    // Check SO_ERROR to confirm the connect succeeded
    let mut so_error: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            &mut so_error as *mut libc::c_int as *mut libc::c_void,
            &mut len,
        )
    };

    if ret < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }

    if so_error != 0 {
        unsafe { libc::close(fd) };
        return Err(io::Error::from_raw_os_error(so_error));
    }

    // Restore to blocking mode
    unsafe { set_blocking(fd)? };

    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}

/// Restore a socket fd to blocking mode.
///
/// # Safety
/// `fd` must be a valid open file descriptor.
unsafe fn set_blocking(fd: libc::c_int) -> io::Result<()> {
    let flags = libc::fcntl(fd, libc::F_GETFL, 0);
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let ret = libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ------------------------------------------------------------------
// Helper: read a full response frame
// ------------------------------------------------------------------

/// Read an `AuthResponse` from the socket stream.
///
/// The wire format is: `[u32 LE version] [u32 LE length] [bincode payload]`.
/// We read the 8-byte header first, then read exactly `length` bytes.
fn read_response(stream: &mut UnixStream) -> Result<AuthResponse, PamModuleError> {
    // Read the fixed 8-byte header
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof
            || e.kind() == io::ErrorKind::ConnectionReset
            || e.kind() == io::ErrorKind::BrokenPipe
        {
            PamModuleError::DaemonUnavailable
        } else {
            PamModuleError::Io(e)
        }
    })?;

    // Read the payload length from bytes 4..8 (after the u32 version)
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

    // Sanity check — response should never be > 1 MiB
    if length > codec::MAX_FRAME_BYTES {
        return Err(PamModuleError::Protocol(format!(
            "response frame too large: {length} bytes"
        )));
    }

    // Read the payload
    let mut payload = vec![0u8; length as usize];
    stream
        .read_exact(&mut payload)
        .map_err(PamModuleError::Io)?;

    // Reassemble full frame (header + payload) and hand to codec::decode
    let mut full = Vec::with_capacity(8 + payload.len());
    full.extend_from_slice(&header);
    full.extend_from_slice(&payload);

    codec::decode::<AuthResponse>(&full).map_err(|e| PamModuleError::Protocol(e.to_string()))
}

// ------------------------------------------------------------------
// Helper: get username from PAM handle
// ------------------------------------------------------------------

/// Retrieve the username from the PAM handle via `pam_get_item(PAM_USER)`.
///
/// # Safety
/// `pamh` must be a valid PAM handle provided by the framework.
unsafe fn get_pam_user(pamh: *mut libc::c_void) -> Result<String, PamModuleError> {
    let mut user_ptr: *const libc::c_char = std::ptr::null();

    // pam_get_item(pamh, PAM_USER, (const void **)&user_ptr)
    let ret = pam_get_item(
        pamh,
        PAM_USER,
        &mut user_ptr as *mut *const libc::c_char as *mut *const libc::c_void,
    );

    if ret != PAM_SUCCESS {
        return Err(PamModuleError::NoUsername);
    }

    if user_ptr.is_null() {
        return Err(PamModuleError::NoUsername);
    }

    // Convert C string to Rust String
    let cstr = std::ffi::CStr::from_ptr(user_ptr);
    cstr.to_str()
        .map(|s| s.to_owned())
        .map_err(|_| PamModuleError::NoUsername)
}

// ------------------------------------------------------------------
// Helpers: syslog
// ------------------------------------------------------------------

/// Write a message to syslog using libc directly.
///
/// We use `LOG_AUTH` facility since this is an authentication module.
fn log_syslog(priority: libc::c_int, message: &str) {
    // Build a NUL-terminated string.
    // We avoid allocating on the heap if possible; fall back if the message
    // doesn't fit in a small stack buffer.
    const MAX: usize = 256;
    let bytes = message.as_bytes();
    let len = bytes.len().min(MAX - 1);

    let mut buf = [0u8; MAX];
    buf[..len].copy_from_slice(&bytes[..len]);
    // buf[len] is already 0 (NUL terminator)

    // SAFETY: buf is a valid NUL-terminated C string.
    unsafe {
        libc::syslog(
            libc::LOG_AUTH | priority,
            b"%s\0".as_ptr() as *const libc::c_char,
            buf.as_ptr() as *const libc::c_char,
        );
    }
}

/// Parsed PAM module runtime options.
#[derive(Debug, Clone, Copy)]
struct ModuleOptions {
    /// If true, daemon/I/O failures return `PAM_IGNORE` instead of `PAM_AUTH_ERR`.
    fail_open: bool,
}

impl Default for ModuleOptions {
    fn default() -> Self {
        Self { fail_open: false }
    }
}

/// Parse PAM module options from `argc`/`argv`.
///
/// Supported option:
/// - `fail_open=1`
/// - `fail_open=true`
fn parse_module_options(argc: i32, argv: *const *const libc::c_char) -> ModuleOptions {
    if argc <= 0 || argv.is_null() {
        return ModuleOptions::default();
    }

    let mut options = ModuleOptions::default();

    for i in 0..argc {
        let arg_ptr = unsafe { *argv.add(i as usize) };
        if arg_ptr.is_null() {
            continue;
        }

        let arg = match unsafe { std::ffi::CStr::from_ptr(arg_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => {
                log_syslog(libc::LOG_WARNING, "dax-auth: ignoring non-utf8 PAM option");
                continue;
            }
        };

        if let Some(value) = arg.strip_prefix("fail_open=") {
            options.fail_open = parse_bool_option(value).unwrap_or_else(|| {
                log_syslog(
                    libc::LOG_WARNING,
                    "dax-auth: invalid fail_open option value, using fail-closed",
                );
                false
            });
        }
    }

    options
}

/// Parse common boolean option forms.
fn parse_bool_option(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("1")
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("on")
    {
        return Some(true);
    }

    if value.eq_ignore_ascii_case("0")
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("off")
    {
        return Some(false);
    }

    None
}

// ------------------------------------------------------------------
// pam_get_item FFI declaration
// ------------------------------------------------------------------

extern "C" {
    /// `int pam_get_item(const pam_handle_t *pamh, int item_type, const void **item)`
    ///
    /// Retrieves a PAM item from the handle. Returns PAM_SUCCESS (0) on success.
    fn pam_get_item(
        pamh: *mut libc::c_void,
        item_type: libc::c_int,
        item: *mut *const libc::c_void,
    ) -> libc::c_int;
}

// ------------------------------------------------------------------
// Error type
// ------------------------------------------------------------------

/// Errors that can occur within the PAM module.
///
/// These are internal — they are translated to PAM return codes at the C boundary.
#[derive(Debug)]
enum PamModuleError {
    /// Cannot get username from PAM (PAM_AUTH_ERR).
    NoUsername,
    /// Daemon socket not available.
    DaemonUnavailable,
    /// Protocol error (PAM_SERVICE_ERR).
    Protocol(String),
    /// I/O error while talking to daemon.
    Io(std::io::Error),
}

impl From<std::io::Error> for PamModuleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// Link against libc for PAM types and syslog
extern crate libc;

// ------------------------------------------------------------------
// Tests
//
// We can't test the real PAM ABI in unit tests (no pamh handle),
// but we can test the codec round-trip and helper logic.
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dax_auth_proto::response::{AuthResponse, AuthResult};
    use uuid::Uuid;

    fn make_granted_frame() -> Vec<u8> {
        let resp = AuthResponse {
            session_id: Uuid::new_v4(),
            version: dax_auth_proto::PROTOCOL_VERSION,
            result: AuthResult::Granted {
                score: 0.92,
                face_index: 0,
            },
            duration_ms: 1234,
        };
        codec::encode(&resp).expect("encode must succeed").to_vec()
    }

    fn make_denied_frame() -> Vec<u8> {
        use dax_auth_proto::response::DenyReason;
        let resp = AuthResponse {
            session_id: Uuid::new_v4(),
            version: dax_auth_proto::PROTOCOL_VERSION,
            result: AuthResult::Denied(DenyReason::BelowThreshold {
                score: 0.4,
                threshold: 0.65,
            }),
            duration_ms: 800,
        };
        codec::encode(&resp).expect("encode must succeed").to_vec()
    }

    #[test]
    fn decode_granted_response() {
        let frame = make_granted_frame();
        let resp = codec::decode::<AuthResponse>(&frame).expect("decode must succeed");
        assert!(resp.is_granted());
    }

    #[test]
    fn decode_denied_response() {
        let frame = make_denied_frame();
        let resp = codec::decode::<AuthResponse>(&frame).expect("decode must succeed");
        assert!(!resp.is_granted());
    }

    #[test]
    fn frame_too_large_rejected() {
        // Craft a frame with an oversized length field
        let mut bad_frame = vec![0u8; 8];
        // version = 1
        bad_frame[0..4].copy_from_slice(&1u32.to_le_bytes());
        // length = MAX_FRAME_BYTES + 1
        let bad_len = codec::MAX_FRAME_BYTES + 1;
        bad_frame[4..8].copy_from_slice(&bad_len.to_le_bytes());
        let result = codec::decode::<AuthResponse>(&bad_frame);
        assert!(result.is_err());
    }

    #[test]
    fn userid_empty_rejected() {
        assert!(UserId::new("").is_err());
    }

    #[test]
    fn userid_too_long_rejected() {
        let long = "a".repeat(257);
        assert!(UserId::new(&long).is_err());
    }

    #[test]
    fn userid_valid() {
        assert!(UserId::new("alice").is_ok());
    }
}
