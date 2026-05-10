use thiserror::Error;

pub type EmbedResult<T> = Result<T, EmbedError>;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("failed to load embedding model: {0}")]
    LoadModel(String),

    #[error("alignment failed: {0}")]
    Alignment(String),

    #[error("warp failed: {0}")]
    Warp(String),

    #[error("model inference failed: {0}")]
    Inference(String),

    #[error("postprocessing failed: {0}")]
    Postprocess(String),

    #[error("comparing embeddings of different lengths: {0} vs {1}")]
    LengthMismatch(usize, usize),
}
