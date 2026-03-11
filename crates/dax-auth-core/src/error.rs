//! Core error types.

use std::path::PathBuf;

/// Errors from the ML inference pipeline.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// ONNX Runtime session error.
    #[error("inference error: {0}")]
    Inference(String),

    /// Model file not found or unreadable.
    #[error("model not found: {path}")]
    ModelNotFound {
        /// Expected model path.
        path: String,
    },

    /// Model file SHA-256 checksum does not match expected value.
    #[error("model file tampered or corrupted: {path}")]
    ModelTampered {
        /// Path of the tampered model file.
        path: PathBuf,
    },

    /// Configuration load error.
    #[error("config load error: {0}")]
    ConfigLoad(String),

    /// Face not detected in the frame.
    #[error("no face detected")]
    NoFaceDetected,

    /// Liveness check determined the face is not live.
    #[error("liveness check failed: {reason}")]
    LivenessFailed {
        /// Reason for failure.
        reason: String,
    },

    /// No enrolled faces for this user.
    #[error("no enrolled faces for user: {user}")]
    NoEnrolledFaces {
        /// Username.
        user: String,
    },

    /// Face store I/O or crypto error.
    #[error("face store error: {0}")]
    Store(String),

    /// Image processing error.
    #[error("image error: {0}")]
    Image(String),

    /// Camera error.
    #[error("camera error: {0}")]
    Camera(#[from] dax_auth_camera::CameraError),
}
