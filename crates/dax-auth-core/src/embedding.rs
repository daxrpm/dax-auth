//! Face embedding types, ArcFace inference, and face alignment.
//!
//! This module implements:
//! - [`FaceEmbedding`]: 512-dim L2-normalised vector, `ZeroizeOnDrop`.
//! - [`FaceRecognizer`]: ArcFace R100 ONNX wrapper.
//! - [`align_face`]: Crop and resize a face region to 112×112.

use crate::detection::DetectedFace;
use crate::CoreError;
use ort::value::TensorRef;
use zeroize::ZeroizeOnDrop;

/// Dimensionality of ArcFace R100 embeddings.
pub const EMBEDDING_DIM: usize = 512;

/// A 512-dimensional face embedding vector.
///
/// The inner `data` values are `ZeroizeOnDrop` — zeroed from memory when this
/// struct is dropped, preventing biometric data leaks.
#[derive(Debug, Clone, ZeroizeOnDrop)]
pub struct FaceEmbedding {
    /// The embedding vector, L2-normalized.
    pub data: Vec<f32>,
}

impl FaceEmbedding {
    /// Create from a raw vector. Applies L2 normalization.
    ///
    /// # Panics
    /// Panics in debug builds if `data.len() != EMBEDDING_DIM`.
    #[must_use]
    pub fn from_raw(mut data: Vec<f32>) -> Self {
        debug_assert_eq!(
            data.len(),
            EMBEDDING_DIM,
            "embedding must be 512-dimensional"
        );
        l2_normalize(&mut data);
        Self { data }
    }

    /// Compute cosine similarity with another embedding.
    ///
    /// Both embeddings must be L2-normalized (which `from_raw` ensures).
    /// For L2-normalized vectors the dot product equals the cosine similarity.
    /// Result is in `[-1.0, 1.0]`, higher = more similar.
    #[must_use]
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        debug_assert_eq!(self.data.len(), other.data.len());
        self.data
            .iter()
            .zip(other.data.iter())
            .map(|(a, b)| a * b)
            .sum()
    }
}

/// Face recognizer using ArcFace R100 ONNX model.
///
/// Accepts a 112×112 aligned RGB face crop and returns a 512-dimensional
/// L2-normalised embedding vector.
pub struct FaceRecognizer {
    session: ort::session::Session,
}

impl FaceRecognizer {
    /// Create a new `FaceRecognizer` from a pre-loaded ONNX session.
    ///
    /// The `session` must correspond to an ArcFace R100 model with
    /// 112×112 input resolution.
    #[must_use]
    pub fn new(session: ort::session::Session) -> Self {
        Self { session }
    }

    /// Align a detected face in `image`, then generate a 512-dim embedding.
    ///
    /// The face is cropped from `image` according to `face.bbox`, resized to
    /// 112×112, and fed into ArcFace R100.  The returned embedding is
    /// L2-normalised and will be zeroed when dropped.
    ///
    /// # Errors
    /// Returns [`CoreError::Inference`] on ONNX session failures.
    /// Returns [`CoreError::Image`] if the RGB buffer is malformed.
    pub fn embed(
        &mut self,
        image: &image::RgbImage,
        face: &DetectedFace,
    ) -> Result<FaceEmbedding, CoreError> {
        let face_112 = align_and_crop(image, face);
        self.embed_aligned(&face_112)
    }

    /// Generate an embedding from an already-aligned 112×112 face image.
    ///
    /// # Errors
    /// Returns [`CoreError::Inference`] on ONNX session failures.
    pub fn embed_aligned(
        &mut self,
        face_112: &image::RgbImage,
    ) -> Result<FaceEmbedding, CoreError> {
        let (tensor_data, shape) = preprocess_arcface(face_112);

        // Use the (shape, &[T]) tuple form — bypasses ndarray version mismatch between
        // workspace ndarray 0.16 and ort rc.12's internal ndarray 0.17.
        let tensor_ref = TensorRef::from_array_view((shape, tensor_data.as_slice()))
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs![tensor_ref])
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        // ArcFace R100 output: "fc1" — shape [1, 512]
        // We index by 0 since the session may name the output "fc1" or by index.
        let embedding_view = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Inference(e.to_string()))?;

        // Flatten [1, 512] → Vec<f32>
        let mut values: Vec<f32> = embedding_view.iter().cloned().collect();
        if values.len() != EMBEDDING_DIM {
            return Err(CoreError::Inference(format!(
                "ArcFace output has {} values, expected {EMBEDDING_DIM}",
                values.len()
            )));
        }

        l2_normalize(&mut values);

        tracing::debug!("face embedding generated");

        Ok(FaceEmbedding { data: values })
    }
}

/// Align a face to the ArcFace standard 112×112 template.
///
/// Phase 1: simplified bbox crop + resize.
/// The face bounding box is cropped from `image` and resized to 112×112.
///
/// # FIXME (Phase 2)
/// Replace with the full 5-point Umeyama similarity transform using
/// `face.keypoints` for ~3–5% better recognition accuracy.
pub fn align_face(
    frame_rgb: &[u8],
    width: u32,
    height: u32,
    face: &DetectedFace,
) -> Result<image::RgbImage, CoreError> {
    let image = image::RgbImage::from_raw(width, height, frame_rgb.to_vec()).ok_or_else(|| {
        CoreError::Image(format!(
            "RGB buffer length {} does not match {width}x{height} frame",
            frame_rgb.len()
        ))
    })?;
    Ok(align_and_crop(&image, face))
}

/// Crop the face bounding box and resize to 112×112.
///
/// Clamps coordinates to the image boundary before cropping.
fn align_and_crop(image: &image::RgbImage, face: &DetectedFace) -> image::RgbImage {
    // TODO (Phase 2): replace with proper 5-point Umeyama similarity transform
    // for better recognition accuracy (~3-5% improvement).
    let [x1, y1, x2, y2] = face.bbox;

    let x1 = x1.max(0.0) as u32;
    let y1 = y1.max(0.0) as u32;
    let x2 = (x2 as u32).min(image.width());
    let y2 = (y2 as u32).min(image.height());
    let w = x2.saturating_sub(x1).max(1);
    let h = y2.saturating_sub(y1).max(1);

    let cropped = image::imageops::crop_imm(image, x1, y1, w, h).to_image();
    image::imageops::resize(&cropped, 112, 112, image::imageops::FilterType::Triangle)
}

/// Preprocess an aligned 112×112 face image for ArcFace inference.
///
/// Normalization: `pixel / 127.5 − 1.0` maps `[0, 255]` → `[−1.0, 1.0]`.
/// Layout: NCHW `[1, 3, 112, 112]` f32.
///
/// Returns `(data_vec, shape)` ready for the `(shape, &[T])` ort tensor API.
fn preprocess_arcface(face_112: &image::RgbImage) -> (Vec<f32>, [usize; 4]) {
    let mut data = vec![0.0_f32; 1 * 3 * 112 * 112];
    for (x, y, pixel) in face_112.enumerate_pixels() {
        let [r, g, b] = pixel.0;
        let channels = [r, g, b];
        for c in 0..3_usize {
            let idx = c * 112 * 112 + y as usize * 112 + x as usize;
            data[idx] = channels[c] as f32 / 127.5 - 1.0;
        }
    }
    (data, [1, 3, 112, 112])
}

/// L2-normalize a vector in-place.
///
/// No-op if the L2 norm is below `1e-8` (zero / near-zero vector).
fn l2_normalize(v: &mut Vec<f32>) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-8 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical() {
        let v: Vec<f32> = (0..EMBEDDING_DIM).map(|i| i as f32).collect();
        let a = FaceEmbedding::from_raw(v.clone());
        let b = FaceEmbedding::from_raw(v);
        let sim = a.cosine_similarity(&b);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "identical embeddings should have sim=1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let mut a_vals = vec![0.0_f32; EMBEDDING_DIM];
        let mut b_vals = vec![0.0_f32; EMBEDDING_DIM];
        a_vals[0] = 1.0;
        b_vals[1] = 1.0;
        // These are already unit vectors, but from_raw normalizes them anyway
        let a = FaceEmbedding { data: a_vals };
        let b = FaceEmbedding { data: b_vals };
        let sim = a.cosine_similarity(&b);
        assert!(
            sim.abs() < 1e-5,
            "orthogonal embeddings should have sim≈0.0, got {sim}"
        );
    }

    #[test]
    fn preprocess_arcface_shape() {
        let img = image::RgbImage::new(112, 112);
        let (data, shape) = preprocess_arcface(&img);
        assert_eq!(shape, [1, 3, 112, 112]);
        assert_eq!(data.len(), 1 * 3 * 112 * 112);
    }

    #[test]
    fn preprocess_arcface_range() {
        // Pixel 128: 128/127.5 - 1.0 ≈ 0.003922
        let img = image::RgbImage::from_pixel(112, 112, image::Rgb([128, 128, 128]));
        let (data, shape) = preprocess_arcface(&img);
        assert_eq!(shape, [1, 3, 112, 112]);
        for &v in &data {
            assert!(
                v > -0.01 && v < 0.02,
                "pixel 128 should normalize near 0, got {v}"
            );
        }
    }

    #[test]
    fn preprocess_arcface_pixel_zero_is_minus_one() {
        let img = image::RgbImage::from_pixel(112, 112, image::Rgb([0, 0, 0]));
        let (data, _) = preprocess_arcface(&img);
        for &v in &data {
            assert!(
                (v - (-1.0)).abs() < 1e-5,
                "pixel 0 should map to -1.0, got {v}"
            );
        }
    }

    #[test]
    fn preprocess_arcface_pixel_255_is_one() {
        let img = image::RgbImage::from_pixel(112, 112, image::Rgb([255, 255, 255]));
        let (data, _) = preprocess_arcface(&img);
        for &v in &data {
            assert!(
                (v - 1.0).abs() < 0.01,
                "pixel 255 should map near 1.0, got {v}"
            );
        }
    }

    #[test]
    fn align_and_crop_produces_112x112() {
        let image = image::RgbImage::from_pixel(640, 480, image::Rgb([100, 100, 100]));
        let face = DetectedFace {
            bbox: [100.0, 100.0, 200.0, 200.0],
            keypoints: [
                [130.0, 130.0],
                [170.0, 130.0],
                [150.0, 150.0],
                [130.0, 170.0],
                [170.0, 170.0],
            ],
            score: 0.99,
        };
        let aligned = align_and_crop(&image, &face);
        assert_eq!(aligned.width(), 112);
        assert_eq!(aligned.height(), 112);
    }

    #[test]
    fn align_face_fn_produces_112x112() {
        let frame_rgb = vec![128u8; 640 * 480 * 3];
        let face = DetectedFace {
            bbox: [100.0, 100.0, 200.0, 200.0],
            keypoints: [
                [130.0, 130.0],
                [170.0, 130.0],
                [150.0, 150.0],
                [130.0, 170.0],
                [170.0, 170.0],
            ],
            score: 0.99,
        };
        let aligned = align_face(&frame_rgb, 640, 480, &face).unwrap();
        assert_eq!(aligned.width(), 112);
        assert_eq!(aligned.height(), 112);
    }

    #[test]
    fn l2_normalize_unit_vector_unchanged() {
        let mut v = vec![0.6_f32, 0.8]; // already unit length: 0.36+0.64=1.0
        l2_normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-5);
        assert!((v[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn l2_normalize_zero_vector_unchanged() {
        let mut v = vec![0.0_f32; 5];
        l2_normalize(&mut v); // should not panic
        assert_eq!(v, vec![0.0_f32; 5]);
    }

    #[test]
    fn from_raw_produces_unit_embedding() {
        let v = vec![1.0_f32; EMBEDDING_DIM];
        let emb = FaceEmbedding::from_raw(v);
        let norm: f32 = emb.data.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "from_raw should produce unit norm, got {norm}"
        );
    }
}
