//! Shared domain types used in both requests and responses.

use serde::{Deserialize, Serialize};
use zeroize::ZeroizeOnDrop;

/// A PAM username — limited to 256 bytes, zeroed on drop.
///
/// Username is PII: we zeroize it as soon as we're done.
#[derive(Debug, Clone, Serialize, Deserialize, ZeroizeOnDrop)]
pub struct UserId(String);

impl UserId {
    /// Create a new `UserId` from a string slice.
    ///
    /// # Errors
    /// Returns `Err` if the username is empty or exceeds 256 bytes.
    pub fn new(name: &str) -> Result<Self, ProtoError> {
        if name.is_empty() {
            return Err(ProtoError::InvalidUsername(
                "username cannot be empty".into(),
            ));
        }
        if name.len() > 256 {
            return Err(ProtoError::InvalidUsername(
                "username exceeds 256 bytes".into(),
            ));
        }
        Ok(Self(name.to_owned()))
    }

    /// Returns the username as a `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Security mode the user has configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityMode {
    /// Balanced security — like Windows Hello.
    /// Liveness + anti-spoofing + threshold 0.65.
    Secure,
    /// Maximum security mode.
    /// Higher threshold (0.72) + stricter liveness + audit logging.
    Paranoid,
}

impl Default for SecurityMode {
    fn default() -> Self {
        Self::Secure
    }
}

/// Errors in the protocol layer.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// Invalid username.
    #[error("invalid username: {0}")]
    InvalidUsername(String),

    /// Serialization/deserialization failure.
    #[error("codec error: {0}")]
    Codec(String),

    /// I/O error on the socket.
    #[error("socket I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Protocol version mismatch between PAM module and daemon.
    #[error("protocol version mismatch: client={client}, daemon={daemon}")]
    VersionMismatch {
        /// Version reported by the PAM client.
        client: u32,
        /// Version expected by the daemon.
        daemon: u32,
    },
}
