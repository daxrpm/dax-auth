//! Liveness detection — anti-spoofing.
//!
//! Two strategies:
//! - **2D (RGB camera)**: MiniFASNetV2 ONNX model — Fourier spectrum analysis.
//! - **IR (infrared camera)**: Not implemented in Phase 1. Returns an error.

use crate::CoreError;
use dax_auth_camera::CameraKind;
use ort::value::TensorRef;

/// Result of a liveness check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LivenessResult {
    /// The face is live (real person).
    Live {
        /// Confidence score (0.0–1.0).
        confidence: f32,
    },
    /// The face appears to be a spoof (photo, screen, mask).
    Spoof {
        /// Spoof confidence score.
        confidence: f32,
    },
}

impl LivenessResult {
    /// Returns `true` if the face passed liveness with the given threshold.
    #[must_use]
    pub fn is_live(&self, threshold: f32) -> bool {
        matches!(self, Self::Live { confidence } if *confidence >= threshold)
    }
}

/// Anti-spoofing detector using MiniFASNetV2 ONNX model (2D/RGB cameras).
///
/// Automatically selects the best strategy based on camera kind.
/// For IR cameras, returns an error in Phase 1 (IR liveness deferred to Phase 2).
pub struct LivenessDetector {
    camera_kind: CameraKind,
    /// ONNX session for 2D anti-spoofing (MiniFASNetV2). `None` for IR-only pipelines.
    anti_spoof_session: Option<ort::session::Session>,
}

impl LivenessDetector {
    /// Create a new `LivenessDetector`.
    ///
    /// # Arguments
    /// * `camera_kind` - Kind of camera providing the face crop.
    /// * `session` - Loaded MiniFASNetV2 ONNX session. Pass `None` for IR-only cameras.
    #[must_use]
    pub fn new(camera_kind: CameraKind, session: Option<ort::session::Session>) -> Self {
        Self {
            camera_kind,
            anti_spoof_session: session,
        }
    }

    /// Returns `true` if a 2D anti-spoofing model session is loaded.
    ///
    /// When `false`, the pipeline should skip the liveness check entirely
    /// (degraded-security mode) rather than treating the missing model as a failure.
    #[must_use]
    pub fn has_model(&self) -> bool {
        self.anti_spoof_session.is_some()
    }

    /// Run liveness detection on the face region.
    ///
    /// For RGB cameras, uses MiniFASNetV2 (2D anti-spoofing).
    /// For IR/combined cameras, returns [`CoreError::LivenessFailed`] in Phase 1.
    ///
    /// # Arguments
    /// * `face_crop` - Packed RGB bytes for the cropped face image (any size — will be resized).
    /// * `ir_frame` - Optional raw IR frame (unused in Phase 1).
    ///
    /// # Errors
    /// Returns `CoreError::LivenessFailed` if IR camera is used (Phase 1 limitation)
    /// or if the ONNX session is unavailable for the 2D path.
    /// A spoofed face returns `Ok(LivenessResult::Spoof { ... })`, not an error.
    pub fn check(
        &mut self,
        face_crop: &[u8],
        ir_frame: Option<&[u8]>,
    ) -> Result<LivenessResult, CoreError> {
        match self.camera_kind {
            CameraKind::Infrared | CameraKind::RgbAndInfrared => self.check_ir(face_crop, ir_frame),
            CameraKind::Rgb => self.check_2d(face_crop),
        }
    }

    /// IR-based liveness — deferred to Phase 2.
    fn check_ir(
        &mut self,
        _face_crop: &[u8],
        _ir_frame: Option<&[u8]>,
    ) -> Result<LivenessResult, CoreError> {
        Err(CoreError::LivenessFailed {
            reason: "IR liveness detection not implemented in Phase 1 — use RGB camera".into(),
        })
    }

    /// 2D liveness using MiniFASNetV2 (Fourier spectrum analysis).
    fn check_2d(&mut self, face_crop: &[u8]) -> Result<LivenessResult, CoreError> {
        let session =
            self.anti_spoof_session
                .as_mut()
                .ok_or_else(|| CoreError::LivenessFailed {
                    reason: "2D anti-spoof session not loaded".into(),
                })?;

        // Determine face crop dimensions: assume square if sqrt works cleanly,
        // otherwise fall back to parsing as a flat RGB buffer.
        // Face crop is raw RGB bytes: len = width * height * 3
        let pixel_count = face_crop.len() / 3;
        let side = (pixel_count as f64).sqrt() as u32;

        // Build an RgbImage from the raw bytes
        let img = image::RgbImage::from_raw(side, side, face_crop.to_vec()).ok_or_else(|| {
            CoreError::Image(format!(
                "face crop buffer length {} does not match expected square RGB image",
                face_crop.len()
            ))
        })?;

        let (tensor_data, shape) = preprocess_minifas(&img);

        // Use the (shape, &[T]) tuple form — bypasses ndarray version mismatch between
        // workspace ndarray 0.16 and ort rc.12's internal ndarray 0.17.
        let tensor_ref = TensorRef::from_array_view((shape, tensor_data.as_slice()))
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        let outputs = session
            .run(ort::inputs![tensor_ref])
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        // MiniFASNetV2 output shape: [1, 3] — (spoof, live, unknown)
        let logits_view = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        // Collect the 3 class logits
        let logits: Vec<f32> = (0..3).map(|c| logits_view[[0, c]]).collect();
        let probs = softmax(&logits);

        // Index 1 = live class
        let live_score = probs[1];

        tracing::debug!(result = live_score >= 0.5_f32, "liveness check complete");

        if live_score >= 0.5 {
            Ok(LivenessResult::Live {
                confidence: live_score,
            })
        } else {
            Ok(LivenessResult::Spoof {
                confidence: 1.0 - live_score,
            })
        }
    }
}

/// Preprocess a face image for MiniFASNetV2 inference.
///
/// Steps:
/// 1. Resize to 80×80 with bilinear interpolation.
/// 2. Normalize: `(pixel/255.0 − mean[c]) / std[c]` using ImageNet statistics.
/// 3. Layout: NCHW `[1, 3, 80, 80]` f32.
///
/// Returns `(data_vec, shape)` ready for the `(shape, &[T])` ort tensor API.
fn preprocess_minifas(image: &image::RgbImage) -> (Vec<f32>, [usize; 4]) {
    let resized = image::imageops::resize(image, 80, 80, image::imageops::FilterType::Triangle);

    // ImageNet normalization constants (RGB order)
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];

    let mut data = vec![0.0_f32; 1 * 3 * 80 * 80];
    for (x, y, pixel) in resized.enumerate_pixels() {
        let [r, g, b] = pixel.0;
        let channels = [r, g, b];
        for c in 0..3_usize {
            let idx = c * 80 * 80 + y as usize * 80 + x as usize;
            data[idx] = (channels[c] as f32 / 255.0 - mean[c]) / std[c];
        }
    }

    (data, [1, 3, 80, 80])
}

/// Numerically stable softmax: `exp(x - max) / sum(exp(x - max))`.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|x| x / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_minifas_shape() {
        let img = image::RgbImage::new(224, 224);
        let (data, shape) = preprocess_minifas(&img);
        assert_eq!(shape, [1, 3, 80, 80]);
        assert_eq!(data.len(), 1 * 3 * 80 * 80);
    }

    #[test]
    fn preprocess_minifas_normalization() {
        // A pixel with R=123 (≈mean[0]*255=123.675) should normalize near 0.0
        let img = image::RgbImage::from_pixel(
            80,
            80,
            image::Rgb([123_u8, 116, 104]), // approx mean * 255
        );
        let (data, _) = preprocess_minifas(&img);
        // Channel 0: (123/255 - 0.485) / 0.229 ≈ small value
        let c0_val = data[0]; // first pixel, channel 0
        assert!(
            c0_val.abs() < 0.1,
            "near-mean pixel should normalize near 0, got {c0_val}"
        );
    }

    #[test]
    fn softmax_sums_to_one() {
        let result = softmax(&[1.0, 2.0, 3.0]);
        let sum: f32 = result.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "softmax should sum to 1.0, got {sum}"
        );
    }

    #[test]
    fn softmax_max_class_is_largest() {
        let result = softmax(&[1.0, 2.0, 3.0]);
        assert!(result[2] > result[1] && result[1] > result[0]);
    }

    #[test]
    fn liveness_result_is_live_checks_threshold() {
        let live = LivenessResult::Live { confidence: 0.8 };
        assert!(live.is_live(0.5));
        assert!(!live.is_live(0.9));

        let spoof = LivenessResult::Spoof { confidence: 0.9 };
        assert!(!spoof.is_live(0.5));
    }

    #[test]
    fn liveness_detector_ir_returns_error() {
        let mut detector = LivenessDetector::new(CameraKind::Infrared, None);
        let result = detector.check(&[128u8; 112 * 112 * 3], None);
        assert!(matches!(result, Err(CoreError::LivenessFailed { .. })));
    }

    #[test]
    fn liveness_detector_rgb_no_session_returns_error() {
        let mut detector = LivenessDetector::new(CameraKind::Rgb, None);
        let result = detector.check(&[128u8; 112 * 112 * 3], None);
        assert!(matches!(result, Err(CoreError::LivenessFailed { .. })));
    }
}
