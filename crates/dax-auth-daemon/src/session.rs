//! Per-connection session handler.
//!
//! Each accepted Unix socket connection maps to one [`SessionHandler`].
//! The session:
//! 1. Reads a framed `AuthRequest` from the socket.
//! 2. Runs the auth pipeline (camera → detection → liveness → recognition).
//! 3. Writes a framed `AuthResponse` back.
//! 4. Closes the connection.
//!
//! Sensitive data (`AuthRequest` with `UserId`) is zeroed on drop via `ZeroizeOnDrop`.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use dax_auth_camera::{CameraDevice, CameraKind};
use dax_auth_core::{AuthPipeline, CoreError};
use dax_auth_proto::{
    codec,
    response::{AuthResult, DenyReason},
    AuthRequest, AuthResponse, SecurityMode, PROTOCOL_VERSION,
};

/// Handles a single IPC connection (one authentication attempt).
///
/// Dropped at end of `handle()`. The contained `AuthRequest` is `ZeroizeOnDrop`
/// — the username is zeroed from memory as soon as the session completes.
pub struct SessionHandler {
    /// Socket stream for this connection.
    stream: UnixStream,
    /// Shared ML pipeline — access serialised via Mutex.
    pipeline: Arc<Mutex<AuthPipeline>>,
    /// Security mode configured by daemon config (authoritative).
    security_mode: SecurityMode,
}

impl SessionHandler {
    /// Create a new session handler for an accepted connection.
    #[must_use]
    pub fn new(
        stream: UnixStream,
        pipeline: Arc<Mutex<AuthPipeline>>,
        security_mode: SecurityMode,
    ) -> Self {
        Self {
            stream,
            pipeline,
            security_mode,
        }
    }

    /// Handle the full request-response lifecycle for this connection.
    ///
    /// # Errors
    /// Returns an error on I/O failures or malformed frames.
    /// Pipeline authentication errors are mapped to denial responses,
    /// not propagated as `Err`.
    pub async fn handle(mut self) -> anyhow::Result<()> {
        // ── 1. Read frame header (8 bytes: version u32 LE + length u32 LE) ───
        let mut header = [0u8; 8];
        self.stream.read_exact(&mut header).await?;

        let length =
            u32::from_le_bytes(header[4..8].try_into().expect("slice is exactly 4 bytes")) as usize;

        if length > codec::MAX_FRAME_BYTES as usize {
            anyhow::bail!(
                "frame too large: {length} bytes (max {})",
                codec::MAX_FRAME_BYTES
            );
        }

        // ── 2. Read the payload ────────────────────────────────────────────────
        let mut frame = vec![0u8; 8 + length];
        frame[..8].copy_from_slice(&header);
        self.stream.read_exact(&mut frame[8..]).await?;

        // ── 3. Decode request ──────────────────────────────────────────────────
        let request: AuthRequest =
            codec::decode(&frame).map_err(|e| anyhow::anyhow!("decode error: {e}"))?;

        let session_id = request.session_id;
        // NOTE: Do NOT log username at debug level — it is PII.
        //       Only the session_id (opaque UUID) is safe to log.
        info!(session_id = %session_id, "auth request received");

        // ── 4. Determine camera kind (best available device) ──────────────────
        let camera_kind = CameraDevice::best_available()
            .map(|d| d.kind)
            .unwrap_or(CameraKind::Rgb);

        // ── 5. Run pipeline (Mutex serialises concurrent auth attempts) ────────
        let pipeline_result = {
            let mut pipeline = self.pipeline.lock().await;
            pipeline
                .authenticate(request.user.as_str(), self.security_mode, camera_kind)
                .await
        };
        // `request` (containing UserId) is dropped here → username zeroed.
        drop(request);

        // ── 6. Map PipelineResult → AuthResult ────────────────────────────────
        let (auth_result, duration_ms) = match pipeline_result {
            Ok(pr) if pr.granted => {
                debug!(session_id = %session_id, "pipeline granted");
                (
                    AuthResult::Granted {
                        score: pr.score.unwrap_or(0.0),
                        face_index: pr.matched_face.unwrap_or(0),
                    },
                    pr.duration_ms,
                )
            }

            Ok(pr) => {
                use dax_auth_core::FailureStage;
                let reason = match pr.failure_stage {
                    Some(FailureStage::NoFaceDetected) => DenyReason::NoFaceDetected,
                    Some(FailureStage::LivenessFailed) => DenyReason::LivenessCheckFailed,
                    Some(FailureStage::BelowThreshold) => DenyReason::BelowThreshold {
                        score: pr.score.unwrap_or(0.0),
                        threshold: pr.threshold,
                    },
                    Some(FailureStage::NoEnrolledFaces) => DenyReason::NoEnrolledFaces,
                    Some(FailureStage::CameraError) => DenyReason::CameraUnavailable,
                    Some(FailureStage::InternalError) | None => DenyReason::InternalError,
                };
                debug!(session_id = %session_id, "pipeline denied");
                (AuthResult::Denied(reason), pr.duration_ms)
            }

            Err(CoreError::NoEnrolledFaces { .. }) => {
                warn!(session_id = %session_id, "no enrolled faces for user");
                (AuthResult::Denied(DenyReason::NoEnrolledFaces), 0)
            }

            Err(CoreError::Camera(_)) => {
                warn!(session_id = %session_id, "camera unavailable");
                (AuthResult::Denied(DenyReason::CameraUnavailable), 0)
            }

            Err(e) => {
                error!(session_id = %session_id, error = %e, "pipeline internal error");
                (AuthResult::Denied(DenyReason::InternalError), 0)
            }
        };

        // ── 7. Build and send response ─────────────────────────────────────────
        let response = AuthResponse {
            session_id,
            version: PROTOCOL_VERSION,
            result: auth_result,
            duration_ms,
        };

        let encoded = codec::encode(&response).map_err(|e| anyhow::anyhow!("encode error: {e}"))?;

        self.stream.write_all(&encoded).await?;
        self.stream.flush().await?;

        info!(
            session_id = %response.session_id,
            granted = response.is_granted(),
            "auth response sent"
        );

        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use dax_auth_proto::{codec, AuthRequest, SecurityMode, UserId};

    #[test]
    fn encode_decode_auth_request_roundtrip() {
        let user = UserId::new("testuser").expect("valid username");
        let req = AuthRequest::new(user, SecurityMode::Secure);
        let encoded = codec::encode(&req).expect("encode");
        let decoded: AuthRequest = codec::decode(&encoded).expect("decode");
        assert_eq!(decoded.user.as_str(), "testuser");
    }
}
