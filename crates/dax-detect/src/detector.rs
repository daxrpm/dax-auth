use std::path::Path;

use dax_capture::Frame;
use ndarray::ArrayView2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tracing::{debug, info};

use crate::decode::{decode_stride, IOU_THRESHOLD, SCORE_THRESHOLD, SCRFD_HEADS};
use crate::error::{DetectError, DetectResult};
use crate::nms::non_maximum_suppression;
use crate::preprocess::preprocess;
use crate::types::FaceDetection;

/// Wraps an ONNX face detector.
pub struct Detector {
    session: Session,
}

impl std::fmt::Debug for Detector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detector").finish_non_exhaustive()
    }
}

impl Detector {
    /// Load a detector model from disk.
    pub fn from_file(path: impl AsRef<Path>) -> DetectResult<Self> {
        let path = path.as_ref();

        let session = Session::builder()
            .map_err(|e| DetectError::LoadModel(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| DetectError::LoadModel(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| DetectError::LoadModel(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| DetectError::LoadModel(e.to_string()))?;

        info!(path = %path.display(), "detector model loaded");
        Ok(Self { session })
    }

    /// Run detection on a single RGB frame and return the surviving
    /// faces in original image coordinates.
    pub fn detect(&mut self, frame: &Frame) -> DetectResult<Vec<FaceDetection>> {
        let (tensor, geometry) = preprocess(frame)?;
        debug!(shape = ?tensor.shape(), "preprocessed tensor");

        let input_value =
            Tensor::from_array(tensor).map_err(|e| DetectError::Inference(e.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![input_value])
            .map_err(|e| DetectError::Inference(e.to_string()))?;

        let mut raw = Vec::new();
        for head in SCRFD_HEADS {
            let scores = extract_2d(&outputs, head.scores)?;
            let bbox = extract_2d(&outputs, head.bbox)?;
            let kps = extract_2d(&outputs, head.kps)?;

            let mut decoded = decode_stride(
                head,
                scores.view(),
                bbox.view(),
                kps.view(),
                geometry,
                SCORE_THRESHOLD,
            );
            debug!(
                stride = head.stride,
                pre_nms = decoded.len(),
                "stride decoded"
            );
            raw.append(&mut decoded);
        }

        let kept = non_maximum_suppression(raw, IOU_THRESHOLD);
        info!(faces = kept.len(), "detection complete");
        Ok(kept)
    }
}

/// Extract a named 2-D `f32` tensor from the session outputs.
fn extract_2d(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> DetectResult<ndarray::Array2<f32>> {
    let value = outputs
        .get(name)
        .ok_or_else(|| DetectError::Postprocess(format!("missing output `{name}`")))?;
    let view = value
        .try_extract_array::<f32>()
        .map_err(|e| DetectError::Postprocess(format!("output `{name}`: {e}")))?;

    let shape = view.shape();
    if shape.len() != 2 {
        return Err(DetectError::Postprocess(format!(
            "output `{name}` expected 2-D, got {shape:?}"
        )));
    }

    let array2: ArrayView2<'_, f32> = view
        .view()
        .into_dimensionality()
        .map_err(|e| DetectError::Postprocess(format!("output `{name}`: {e}")))?;
    Ok(array2.to_owned())
}
