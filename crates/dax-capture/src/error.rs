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
}
