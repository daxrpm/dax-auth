// Tensor packing uses lossy integer↔float casts that are well-bounded
// (target_size ≤ 256, channels = 3).
#![allow(clippy::cast_precision_loss)]

use std::path::Path;

use dax_capture::Frame;
use dax_detect::Bbox;
use ndarray::Array4;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tracing::{debug, info, trace};

use crate::crop::crop_face_to_bgr;
use crate::error::{LivenessError, LivenessResult};

/// `MiniFASNetV2` was trained with this crop expansion factor; it
/// matches the `Silent-Face` reference implementation.
const DEFAULT_SCALE: f32 = 2.7;

/// Index of the "real" class in the model's output logits.
///
/// Silent-Face `MiniFASNet` is a 3-class classifier (print spoof /
/// real / replay spoof). Class 1 is the live one; everything else
/// collapses into a single "spoof" probability for downstream code.
const REAL_CLASS_INDEX: usize = 1;

/// Verdict returned by the liveness model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessVerdict {
    Real,
    Fake,
}

impl LivenessVerdict {
    #[must_use]
    pub fn is_real(self) -> bool {
        matches!(self, Self::Real)
    }
}

/// Outcome of a single liveness inference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivenessReport {
    pub verdict: LivenessVerdict,
    pub real_prob: f32,
    pub spoof_prob: f32,
}

impl LivenessReport {
    /// Confidence of the chosen verdict in `[0, 1]`.
    #[must_use]
    pub fn score(&self) -> f32 {
        match self.verdict {
            LivenessVerdict::Real => self.real_prob,
            LivenessVerdict::Fake => self.spoof_prob,
        }
    }
}

/// Wraps the `MiniFASNetV2` ONNX model.
pub struct LivenessChecker {
    session: Session,
    input_size: u32,
    scale: f32,
}

impl std::fmt::Debug for LivenessChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivenessChecker")
            .field("input_size", &self.input_size)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

impl LivenessChecker {
    pub fn from_file(path: impl AsRef<Path>) -> LivenessResult<Self> {
        let path = path.as_ref();
        let session = Session::builder()
            .map_err(|e| LivenessError::LoadModel(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| LivenessError::LoadModel(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| LivenessError::LoadModel(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| LivenessError::LoadModel(e.to_string()))?;

        let input_size = read_input_size(&session)?;
        info!(
            path = %path.display(),
            input_size,
            "liveness model loaded"
        );

        Ok(Self {
            session,
            input_size,
            scale: DEFAULT_SCALE,
        })
    }

    pub fn check(&mut self, frame: &Frame, bbox: &Bbox) -> LivenessResult<LivenessReport> {
        let bgr = crop_face_to_bgr(frame, bbox, self.scale, self.input_size)?;
        let tensor = pack_tensor(&bgr, self.input_size);
        let input_value =
            Tensor::from_array(tensor).map_err(|e| LivenessError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs![input_value])
            .map_err(|e| LivenessError::Inference(e.to_string()))?;

        let (_name, value) = outputs
            .iter()
            .next()
            .ok_or_else(|| LivenessError::Postprocess(String::from("no outputs")))?;
        let view = value
            .try_extract_array::<f32>()
            .map_err(|e| LivenessError::Postprocess(e.to_string()))?;
        let output_shape = view.shape().to_vec();
        let logits: Vec<f32> = view.iter().copied().collect();
        trace!(?output_shape, ?logits, "raw model output");
        if logits.len() < 2 {
            return Err(LivenessError::Postprocess(format!(
                "expected ≥2 logits, got {}",
                logits.len()
            )));
        }

        let probs = softmax(&logits);
        trace!(?probs, "post-softmax");

        let real_prob = probs.get(REAL_CLASS_INDEX).copied().unwrap_or(0.0);
        let spoof_prob: f32 = probs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != REAL_CLASS_INDEX)
            .map(|(_, p)| *p)
            .sum();
        let verdict = if real_prob > spoof_prob {
            LivenessVerdict::Real
        } else {
            LivenessVerdict::Fake
        };
        debug!(real_prob, spoof_prob, ?verdict, "liveness inference");

        Ok(LivenessReport {
            verdict,
            real_prob,
            spoof_prob,
        })
    }
}

fn read_input_size(session: &Session) -> LivenessResult<u32> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| LivenessError::InvalidInputShape(String::from("model has no inputs")))?;
    let dims: &[i64] = match input.dtype() {
        ort::value::ValueType::Tensor { shape, .. } => shape,
        other => {
            return Err(LivenessError::InvalidInputShape(format!(
                "non-tensor input: {other:?}"
            )));
        }
    };
    if dims.len() != 4 {
        return Err(LivenessError::InvalidInputShape(format!(
            "expected NCHW, got shape {dims:?}"
        )));
    }
    let h = dims[2];
    let w = dims[3];
    if h <= 0 || w <= 0 || h != w {
        return Err(LivenessError::InvalidInputShape(format!(
            "expected square H==W, got {h}x{w}"
        )));
    }
    u32::try_from(h).map_err(|_| {
        LivenessError::InvalidInputShape(format!("input size {h} does not fit in u32"))
    })
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&v| (v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum <= 0.0 {
        return vec![0.0; logits.len()];
    }
    exps.into_iter().map(|v| v / sum).collect()
}

fn pack_tensor(bgr: &[u8], size: u32) -> Array4<f32> {
    let s = size as usize;
    let mut tensor = Array4::<f32>::zeros((1, 3, s, s));
    for y in 0..s {
        for x in 0..s {
            let idx = (y * s + x) * 3;
            tensor[[0, 0, y, x]] = f32::from(bgr[idx]);
            tensor[[0, 1, y, x]] = f32::from(bgr[idx + 1]);
            tensor[[0, 2, y, x]] = f32::from(bgr[idx + 2]);
        }
    }
    tensor
}
