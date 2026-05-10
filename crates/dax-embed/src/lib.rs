//! Face embeddings and similarity.
//!
//! Pipeline:
//! 1. Five SCRFD landmarks → similarity transform via `Umeyama`
//! 2. Affine warp to a 112×112 RGB canvas aligned to `ArcFace` canon
//! 3. ONNX inference → 512-D float vector
//! 4. L2-normalise → [`Embedding`]
//!
//! Two [`Embedding`]s are compared via [`Embedding::cosine`].

mod align;
mod embedder;
mod embedding;
mod error;
mod warp;

pub use align::{estimate_alignment, AffineTransform, ALIGNED_SIZE};
pub use embedder::Embedder;
pub use embedding::Embedding;
pub use error::{EmbedError, EmbedResult};
pub use warp::warp_aligned;
