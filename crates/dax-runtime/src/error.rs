use thiserror::Error;

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("vault: {0}")]
    Vault(#[from] dax_store::StoreError),

    #[error("capture: {0}")]
    Capture(#[from] dax_capture::CaptureError),

    #[error("detection: {0}")]
    Detect(#[from] dax_detect::DetectError),

    #[error("embedding: {0}")]
    Embed(#[from] dax_embed::EmbedError),

    #[error("liveness: {0}")]
    Liveness(#[from] dax_liveness::LivenessError),

    #[error("user `{0}` is not enrolled")]
    UserNotEnrolled(String),

    #[error("user `{0}` has no templates")]
    EmptyTemplates(String),

    #[error("no face detected in capture")]
    NoFace,

    #[error("config: {0}")]
    Config(String),
}
