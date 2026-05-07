use thiserror::Error;

pub type CaptureResult<T> = Result<T, CaptureError>;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("camera backend failed to initialise: {0}")]
    BackendInit(String),

    #[error("failed to enumerate camera devices: {0}")]
    Enumerate(String),

    #[error("device {0} not found")]
    DeviceNotFound(String),

    #[error("failed to open camera device: {0}")]
    DeviceOpen(String),

    #[error("camera stream error: {0}")]
    Stream(String),

    #[error("failed to decode captured frame: {0}")]
    Decode(String),
}
