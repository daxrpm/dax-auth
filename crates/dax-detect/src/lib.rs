//! Face detection.
//!
//! The current backend wraps the SCRFD ONNX model from `InsightFace`,
//! loaded through `ort`. The public surface is intentionally narrow:
//! load a [`Detector`] once, then call [`Detector::detect`] on every
//! frame.

mod decode;
mod detector;
mod error;
mod nms;
mod preprocess;
mod types;

pub use detector::Detector;
pub use error::{DetectError, DetectResult};
pub use types::{Bbox, FaceDetection, Landmarks, Point2D};
