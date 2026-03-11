//! Camera error types.

/// Errors from camera operations.
#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    /// No camera device found at the given path.
    #[error("camera device not found: {path}")]
    DeviceNotFound {
        /// Device path (e.g., `/dev/video0`).
        path: String,
    },

    /// Failed to open or query the camera device.
    #[error("failed to open camera {path}: {source}")]
    OpenFailed {
        /// Device path.
        path: String,
        /// Underlying error.
        source: std::io::Error,
    },

    /// The camera does not support the required format.
    #[error("unsupported pixel format: {format}")]
    UnsupportedFormat {
        /// Format fourcc string (e.g., "YUYV").
        format: String,
    },

    /// Failed to capture a frame.
    #[error("frame capture failed: {0}")]
    CaptureFailed(String),

    /// Frame decoding error.
    #[error("frame decode failed: {0}")]
    DecodeFailed(String),

    /// No usable (non-black) frame was captured within the attempt limit.
    #[error("no usable frame captured after {attempts} attempts")]
    NoUsableFrame {
        /// Number of capture attempts made.
        attempts: u32,
    },

    /// The camera read timed out waiting for a frame.
    #[error("camera capture timeout")]
    Timeout,

    /// A V4L2 subsystem error.
    #[error("V4L2 error: {0}")]
    V4l2(String),
}
