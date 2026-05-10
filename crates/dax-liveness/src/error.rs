use thiserror::Error;

pub type LivenessResult<T> = Result<T, LivenessError>;

#[derive(Debug, Error)]
pub enum LivenessError {
    #[error("failed to load liveness model: {0}")]
    LoadModel(String),

    #[error("model has unexpected input shape: {0}")]
    InvalidInputShape(String),

    #[error("preprocessing failed: {0}")]
    Preprocess(String),

    #[error("model inference failed: {0}")]
    Inference(String),

    #[error("postprocessing failed: {0}")]
    Postprocess(String),
}
