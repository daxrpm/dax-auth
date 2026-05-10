use thiserror::Error;

pub type DetectResult<T> = Result<T, DetectError>;

#[derive(Debug, Error)]
pub enum DetectError {
    #[error("failed to load detector model: {0}")]
    LoadModel(String),

    #[error("preprocessing failed: {0}")]
    Preprocess(String),

    #[error("model inference failed: {0}")]
    Inference(String),

    #[error("postprocessing failed: {0}")]
    Postprocess(String),
}
