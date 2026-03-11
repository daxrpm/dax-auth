//! # dax-auth-proto
//!
//! IPC protocol types shared between `pam_dax_auth.so` and `dax-authd`.
//!
//! ## Design principles
//! - All types are `#[derive(Serialize, Deserialize)]` for bincode encoding
//! - Sensitive data types implement `ZeroizeOnDrop`
//! - The protocol is versioned to allow daemon/PAM module to evolve independently
//! - Wire format: `[u32 length (LE)] + [bincode payload]`

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![warn(clippy::pedantic)]

pub mod codec;
pub mod request;
pub mod response;
pub mod types;

pub use request::AuthRequest;
pub use response::AuthResponse;
pub use types::{SecurityMode, UserId};

/// Current IPC protocol version. PAM module and daemon MUST agree on this.
pub const PROTOCOL_VERSION: u32 = 1;

/// Path to the Unix domain socket.
/// Daemon creates it; PAM module connects to it.
pub const SOCKET_PATH: &str = "/run/dax-auth/daemon.sock";
