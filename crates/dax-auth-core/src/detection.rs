//! Face detection with RetinaFace ONNX.

use crate::CoreError;
use ndarray::Array4;
use ort::value::TensorRef;

/// A detected face bounding box with keypoints.
#[derive(Debug, Clone)]
pub struct DetectedFace {
    /// Bounding box: [x1, y1, x2, y2] in pixels.
    pub bbox: [f32; 4],
    /// Five facial keypoints: [left_eye, right_eye, nose, left_mouth, right_mouth]
    /// Each is [x, y] in pixels.
    pub keypoints: [[f32; 2]; 5],
    /// Detection confidence score (0.0–1.0).
    pub score: f32,
}

/// Face detector using RetinaFace ONNX model.
pub struct FaceDetector {
    session: ort::session::Session,
    /// Input size expected by the model: (width, height).
    input_size: (u32, u32),
}

impl FaceDetector {
    /// Create a new `FaceDetector` from a pre-loaded ONNX session.
    ///
    /// The `session` must correspond to a RetinaFace model with
    /// 640×640 input resolution.
    #[must_use]
    pub fn new(session: ort::session::Session) -> Self {
        Self {
            session,
            input_size: (640, 640),
        }
    }

    /// Detect all faces in a frame.
    ///
    /// Returns faces sorted by confidence (highest first).
    /// Only returns faces with score >= `min_confidence`.
    ///
    /// The `frame_rgb` slice must contain `width * height * 3` packed RGB bytes
    /// in row-major order.
    ///
    /// # Errors
    /// Returns [`CoreError::Inference`] if the ONNX session fails.
    /// Returns [`CoreError::Image`] if the RGB buffer is invalid.
    pub fn detect(
        &mut self,
        frame_rgb: &[u8],
        width: u32,
        height: u32,
        min_confidence: f32,
    ) -> Result<Vec<DetectedFace>, CoreError> {
        // Build an RgbImage from the raw bytes
        let image =
            image::RgbImage::from_raw(width, height, frame_rgb.to_vec()).ok_or_else(|| {
                CoreError::Image(format!(
                    "RGB buffer length {} does not match {}x{} frame",
                    frame_rgb.len(),
                    width,
                    height
                ))
            })?;

        // Track scale factors so we can map 640×640 coords back to original dims
        let (target_w, target_h) = self.input_size;
        let scale_x = width as f32 / target_w as f32;
        let scale_y = height as f32 / target_h as f32;

        // Preprocess: resize → BGR mean subtraction → NCHW tensor
        let tensor = preprocess_retinaface(&image, self.input_size);

        // Run inference.
        // NOTE: The workspace uses ndarray 0.16 but ort's TensorArrayData trait is implemented
        // for ndarray 0.17 types (different crate versions — not compatible at the type level).
        // We use the (shape, &[T]) form which works without any ndarray version dependency.
        let tensor_data: &[f32] = tensor
            .as_slice()
            .ok_or_else(|| CoreError::Inference("tensor has non-contiguous layout".into()))?;
        let shape = [1_usize, 3, target_h as usize, target_w as usize];
        let tensor_ref = TensorRef::from_array_view((shape, tensor_data))
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs![tensor_ref])
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        // Extract the 3 output tensors by index
        // RetinaFace ONNX Model Zoo outputs: boxes [1,N,4], scores [1,N,2], landmarks [1,N,10]
        let boxes_view = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Inference(e.to_string()))?;
        let scores_view = outputs[1]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Inference(e.to_string()))?;
        let points_view = outputs[2]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        let boxes_shape = boxes_view.shape();
        let n = if boxes_shape.len() >= 2 {
            boxes_shape[boxes_shape.len() - 2]
        } else {
            0
        };

        // Generate anchors for anchor-box decoding
        let anchors = generate_anchors(target_w);

        // Decode detections
        let mut candidates: Vec<DetectedFace> = Vec::new();

        for i in 0..n {
            // Score: index 1 = face class probability
            let face_score = scores_view[[0, i, 1]];
            if face_score < min_confidence {
                continue;
            }

            let anchor = anchors[i];
            let (acx, acy, aw, ah) = (anchor[0], anchor[1], anchor[2], anchor[3]);

            // Box decoding: offsets relative to anchor
            let dx = boxes_view[[0, i, 0]];
            let dy = boxes_view[[0, i, 1]];
            let dw = boxes_view[[0, i, 2]];
            let dh = boxes_view[[0, i, 3]];

            let cx = acx + dx * aw;
            let cy = acy + dy * ah;
            let w = aw * dw.exp();
            let h = ah * dh.exp();

            // Convert center format → corner format, scaled to 640×640 already
            let x1 = (cx - w / 2.0).max(0.0);
            let y1 = (cy - h / 2.0).max(0.0);
            let x2 = (cx + w / 2.0).min(target_w as f32);
            let y2 = (cy + h / 2.0).min(target_h as f32);

            // Scale back to original image coordinates
            let x1 = x1 * scale_x;
            let y1 = y1 * scale_y;
            let x2 = x2 * scale_x;
            let y2 = y2 * scale_y;

            // Decode 5 facial keypoints (10 values: x0,y0,x1,y1,...x4,y4)
            let mut keypoints = [[0.0f32; 2]; 5];
            for k in 0..5 {
                let px = points_view[[0, i, k * 2]];
                let py = points_view[[0, i, k * 2 + 1]];
                // Keypoints are also anchor-relative offsets
                let kx = (acx + px * aw) * scale_x;
                let ky = (acy + py * ah) * scale_y;
                keypoints[k] = [kx, ky];
            }

            candidates.push(DetectedFace {
                bbox: [x1, y1, x2, y2],
                keypoints,
                score: face_score.clamp(0.0, 1.0),
            });
        }

        // Apply Non-Maximum Suppression
        let mut faces = nms(candidates, 0.4);

        // Sort by confidence descending
        faces.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::debug!(count = faces.len(), "faces detected");

        Ok(faces)
    }
}

/// Preprocess an RGB image for RetinaFace inference.
///
/// Steps:
/// 1. Resize to `target_size` (bilinear interpolation)
/// 2. Convert RGB → BGR
/// 3. Subtract ImageNet BGR mean: [104.0, 117.0, 123.0]
/// 4. Layout: NCHW `[1, 3, H, W]` f32
fn preprocess_retinaface(image: &image::RgbImage, target_size: (u32, u32)) -> Array4<f32> {
    let (target_w, target_h) = target_size;

    let resized = image::imageops::resize(
        image,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );

    // BGR mean values (used because RetinaFace was trained on BGR images)
    let mean_bgr = [104.0_f32, 117.0, 123.0];

    let mut tensor = Array4::<f32>::zeros((1, 3, target_h as usize, target_w as usize));
    for (x, y, pixel) in resized.enumerate_pixels() {
        let [r, g, b] = pixel.0;
        // Channel layout: C=0 → B, C=1 → G, C=2 → R  (BGR order with mean subtraction)
        tensor[[0, 0, y as usize, x as usize]] = b as f32 - mean_bgr[0];
        tensor[[0, 1, y as usize, x as usize]] = g as f32 - mean_bgr[1];
        tensor[[0, 2, y as usize, x as usize]] = r as f32 - mean_bgr[2];
    }
    tensor
}

/// Generate RetinaFace anchor boxes for a square input of `input_size` pixels.
///
/// Returns anchors as `[cx, cy, w, h]` in the same order as model outputs.
///
/// Configuration matches the ONNX Model Zoo RetinaFace training:
/// - Strides: `[8, 16, 32]`
/// - Anchor sizes per stride: `[[16, 32], [64, 128], [256, 512]]`
/// - Total for 640×640 input: `(80*80 + 40*40 + 20*20) * 2 = 16800` anchors
fn generate_anchors(input_size: u32) -> Vec<[f32; 4]> {
    let strides = [8_u32, 16, 32];
    let anchor_sizes = [[16.0_f32, 32.0], [64.0, 128.0], [256.0, 512.0]];
    let mut anchors: Vec<[f32; 4]> = Vec::new();

    for (stride, sizes) in strides.iter().zip(anchor_sizes.iter()) {
        let feat_h = input_size / stride;
        let feat_w = input_size / stride;
        for gy in 0..feat_h {
            for gx in 0..feat_w {
                let cx = (gx as f32 + 0.5) * *stride as f32;
                let cy = (gy as f32 + 0.5) * *stride as f32;
                for &size in sizes.iter() {
                    anchors.push([cx, cy, size, size]);
                }
            }
        }
    }

    anchors
}

/// Compute Intersection-over-Union for two bounding boxes `[x1, y1, x2, y2]`.
fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = a[2].min(b[2]);
    let y2 = a[3].min(b[3]);

    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    if intersection == 0.0 {
        return 0.0;
    }

    let area_a = (a[2] - a[0]) * (a[3] - a[1]);
    let area_b = (b[2] - b[0]) * (b[3] - b[1]);
    intersection / (area_a + area_b - intersection)
}

/// Apply Non-Maximum Suppression to remove overlapping detections.
///
/// Candidates must be sorted by score descending before calling (or sorting
/// is done inside).  Keeps the highest-scoring box among any group of boxes
/// with IoU > `iou_threshold`.
fn nms(mut candidates: Vec<DetectedFace>, iou_threshold: f32) -> Vec<DetectedFace> {
    // Sort descending by score so we always keep the best box
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep: Vec<DetectedFace> = Vec::new();

    'outer: for candidate in candidates {
        for kept in &keep {
            if iou(&candidate.bbox, &kept.bbox) > iou_threshold {
                continue 'outer;
            }
        }
        keep.push(candidate);
    }

    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_shape_is_correct() {
        let img = image::RgbImage::new(1920, 1080);
        let tensor = preprocess_retinaface(&img, (640, 640));
        assert_eq!(tensor.shape(), &[1, 3, 640, 640]);
    }

    #[test]
    fn preprocess_mean_subtraction() {
        // Pure blue pixel (R=0, G=0, B=255) in RGB
        // After RGB→BGR: B_channel=255, G_channel=0, R_channel=0
        // C=0 (B channel) = 255 - 104.0 = 151.0
        // C=1 (G channel) =   0 - 117.0 = -117.0
        // C=2 (R channel) =   0 - 123.0 = -123.0
        let mut img = image::RgbImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgb([0, 0, 255]));
        let t = preprocess_retinaface(&img, (1, 1));
        assert!(
            (t[[0, 0, 0, 0]] - 151.0).abs() < 1.0,
            "B channel wrong: got {}",
            t[[0, 0, 0, 0]]
        );
        assert!(
            (t[[0, 1, 0, 0]] - (-117.0)).abs() < 1.0,
            "G channel wrong: got {}",
            t[[0, 1, 0, 0]]
        );
        assert!(
            (t[[0, 2, 0, 0]] - (-123.0)).abs() < 1.0,
            "R channel wrong: got {}",
            t[[0, 2, 0, 0]]
        );
    }

    #[test]
    fn generate_anchors_correct_count() {
        let anchors = generate_anchors(640);
        // (80*80 + 40*40 + 20*20) * 2 = 16800
        assert_eq!(anchors.len(), 16800);
    }

    #[test]
    fn iou_identical_boxes_is_one() {
        let a = [0.0_f32, 0.0, 10.0, 10.0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn iou_non_overlapping_boxes_is_zero() {
        let a = [0.0_f32, 0.0, 5.0, 5.0];
        let b = [10.0_f32, 10.0, 20.0, 20.0];
        assert_eq!(iou(&a, &b), 0.0);
    }

    #[test]
    fn nms_removes_duplicate_boxes() {
        let a = DetectedFace {
            bbox: [0.0, 0.0, 10.0, 10.0],
            keypoints: [[0.0; 2]; 5],
            score: 0.9,
        };
        let b = DetectedFace {
            bbox: [0.5, 0.5, 10.5, 10.5], // nearly identical
            keypoints: [[0.0; 2]; 5],
            score: 0.8,
        };
        let kept = nms(vec![a, b], 0.4);
        assert_eq!(kept.len(), 1, "duplicate box should be suppressed");
        assert!((kept[0].score - 0.9).abs() < 1e-5, "highest score kept");
    }

    #[test]
    #[ignore = "requires retinaface_10g.onnx model file"]
    fn detect_returns_empty_for_blank_frame() {
        // Load model from env var path, run on blank image, expect Ok(vec![])
    }
}
