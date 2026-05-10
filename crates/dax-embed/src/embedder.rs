// Numeric casts in tensor packing are intentional and well-bounded.
#![allow(clippy::cast_precision_loss)]

use std::path::Path;

use dax_capture::Frame;
use dax_detect::Landmarks;
use ndarray::Array4;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tracing::{debug, info, trace};

use crate::align::{estimate_alignment, ALIGNED_SIZE};
use crate::embedding::Embedding;
use crate::error::{EmbedError, EmbedResult};
use crate::warp::warp_aligned;

/// Per-channel normalisation used by the `InsightFace` recognition
/// models. NOTE: SCRFD uses std=128 but the recognition stack uses
/// std=127.5 — the asymmetry comes from the original training scripts.
const MEAN: f32 = 127.5;
const STD: f32 = 127.5;

/// Wraps an ONNX face-recognition model.
pub struct Embedder {
    session: Session,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder").finish_non_exhaustive()
    }
}

impl Embedder {
    pub fn from_file(path: impl AsRef<Path>) -> EmbedResult<Self> {
        let path = path.as_ref();
        let session = Session::builder()
            .map_err(|e| EmbedError::LoadModel(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| EmbedError::LoadModel(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| EmbedError::LoadModel(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| EmbedError::LoadModel(e.to_string()))?;
        info!(path = %path.display(), "embedder model loaded");
        for input in session.inputs() {
            info!(name = %input.name(), "model input");
        }
        for output in session.outputs() {
            info!(name = %output.name(), "model output");
        }
        Ok(Self { session })
    }

    /// Align, warp, run inference and L2-normalise to produce a
    /// single embedding for the given face.
    pub fn embed(&mut self, frame: &Frame, landmarks: &Landmarks) -> EmbedResult<Embedding> {
        trace!(
            le = ?(landmarks.left_eye.x, landmarks.left_eye.y),
            re = ?(landmarks.right_eye.x, landmarks.right_eye.y),
            no = ?(landmarks.nose.x, landmarks.nose.y),
            lm = ?(landmarks.left_mouth.x, landmarks.left_mouth.y),
            rm = ?(landmarks.right_mouth.x, landmarks.right_mouth.y),
            "raw landmarks"
        );
        let transform = estimate_alignment(landmarks)?;
        trace!(a = ?transform.a, t = ?transform.t, "alignment transform");
        let aligned = warp_aligned(frame, &transform)?;
        debug!(
            bytes = aligned.len(),
            side = ALIGNED_SIZE,
            "aligned face built"
        );

        let tensor = pack_tensor(&aligned);
        let input_value =
            Tensor::from_array(tensor).map_err(|e| EmbedError::Inference(e.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![input_value])
            .map_err(|e| EmbedError::Inference(e.to_string()))?;

        let (_name, value) = outputs
            .iter()
            .next()
            .ok_or_else(|| EmbedError::Postprocess(String::from("model produced no outputs")))?;
        let view = value
            .try_extract_array::<f32>()
            .map_err(|e| EmbedError::Postprocess(e.to_string()))?;

        let raw: Vec<f32> = view.iter().copied().collect();
        debug!(dims = raw.len(), "raw embedding extracted");
        Embedding::from_raw(raw)
    }
}

/// Convert an aligned 112×112 packed-RGB buffer into the (1, 3, 112,
/// 112) NCHW tensor expected by the recognition network.
fn pack_tensor(aligned: &[u8]) -> Array4<f32> {
    let size = ALIGNED_SIZE as usize;
    let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 3;
            tensor[[0, 0, y, x]] = (f32::from(aligned[idx]) - MEAN) / STD;
            tensor[[0, 1, y, x]] = (f32::from(aligned[idx + 1]) - MEAN) / STD;
            tensor[[0, 2, y, x]] = (f32::from(aligned[idx + 2]) - MEAN) / STD;
        }
    }
    tensor
}
