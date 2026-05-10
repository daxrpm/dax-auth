use crate::error::{EmbedError, EmbedResult};

/// A face embedding stored as an L2-normalised float vector.
///
/// Keeping the L2 norm invariant means [`Embedding::cosine`] is just
/// the dot product — no per-call normalisation, and downstream code
/// never has to worry about denormalised vectors leaking in.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    data: Vec<f32>,
}

impl Embedding {
    /// Construct from a raw vector, applying L2 normalisation.
    pub fn from_raw(mut data: Vec<f32>) -> EmbedResult<Self> {
        if data.is_empty() {
            return Err(EmbedError::Postprocess(String::from(
                "empty embedding vector",
            )));
        }
        let norm = data.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm <= f32::EPSILON {
            return Err(EmbedError::Postprocess(String::from(
                "embedding has zero norm",
            )));
        }
        for v in &mut data {
            *v /= norm;
        }
        Ok(Self { data })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Cosine similarity with another embedding. Both vectors are
    /// already L2-normalised so this collapses to a dot product.
    pub fn cosine(&self, other: &Self) -> EmbedResult<f32> {
        if self.data.len() != other.data.len() {
            return Err(EmbedError::LengthMismatch(
                self.data.len(),
                other.data.len(),
            ));
        }
        Ok(self.data.iter().zip(&other.data).map(|(a, b)| a * b).sum())
    }
}
