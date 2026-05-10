//! Passive liveness / anti-spoofing checks.
//!
//! Wraps the `MiniFASNetV2` ONNX model from the `Silent-Face`
//! family. Given a frame and a [`dax_detect::Bbox`], runs the model
//! and returns a [`LivenessReport`] with a `Real` / `Fake` verdict
//! and the underlying class probabilities.

mod checker;
mod crop;
mod error;

pub use checker::{LivenessChecker, LivenessReport, LivenessVerdict};
pub use error::{LivenessError, LivenessResult};
