//! Auth response sent from daemon → PAM module.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Reason for a denial — returned to PAM for logging (NOT shown to user).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DenyReason {
    /// No face detected within the timeout window.
    NoFaceDetected,
    /// Face detected but liveness check failed (possible spoof attempt).
    LivenessCheckFailed,
    /// Face detected and live but similarity below threshold.
    BelowThreshold {
        /// Similarity score achieved (0.0–1.0).
        score: f32,
        /// Threshold required.
        threshold: f32,
    },
    /// No enrolled faces for this user.
    NoEnrolledFaces,
    /// Maximum attempts exceeded.
    MaxAttemptsExceeded,
    /// Daemon internal error — details logged server-side, not exposed to client.
    InternalError,
    /// Camera not available or failed to open.
    CameraUnavailable,
}

/// Result of an authentication attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthResult {
    /// Authentication succeeded.
    Granted {
        /// Similarity score that triggered the grant (0.0–1.0).
        score: f32,
        /// Which enrolled face matched (index).
        face_index: usize,
    },
    /// Authentication denied.
    Denied(DenyReason),
}

/// Full response envelope from daemon → PAM module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    /// Session ID echoed from the request.
    pub session_id: Uuid,

    /// Protocol version of the daemon.
    pub version: u32,

    /// The authentication result.
    pub result: AuthResult,

    /// Duration of the auth pipeline in milliseconds (for telemetry/logging).
    pub duration_ms: u64,
}

impl AuthResponse {
    /// Returns `true` if authentication was granted.
    #[must_use]
    pub fn is_granted(&self) -> bool {
        matches!(self.result, AuthResult::Granted { .. })
    }
}
