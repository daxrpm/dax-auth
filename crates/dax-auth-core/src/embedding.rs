//! Face embedding types, ArcFace inference, and face alignment.
//!
//! This module implements:
//! - [`FaceEmbedding`]: 512-dim L2-normalised vector, `ZeroizeOnDrop`.
//! - [`FaceRecognizer`]: ArcFace R100 ONNX wrapper.
//! - [`align_face`]: Align a face to 112×112 via Umeyama similarity transform.
//!
//! ## Face alignment
//!
//! [`align_face`] applies the Umeyama (1991) 2-D similarity transform when the
//! detection confidence is ≥ 0.3, mapping the five detector keypoints to the
//! canonical ArcFace 112×112 template ([`ARCFACE_TEMPLATE_112`]).  For very
//! low-confidence detections the legacy bbox-crop fallback is used instead.

use crate::detection::DetectedFace;
use crate::CoreError;
use ort::value::TensorRef;
use zeroize::ZeroizeOnDrop;

// ─── ArcFace canonical landmark template ──────────────────────────────────────

/// Canonical face landmark positions for 112×112 ArcFace input.
///
/// Order: `[left_eye, right_eye, nose_tip, left_mouth_corner, right_mouth_corner]`.
///
/// These coordinates originate from the InsightFace / ArcFace papers and are
/// the standard target used when warping a detected face into the model's input
/// space.  Any face aligned to these points and fed into ArcFace R100 will
/// produce embeddings in the expected metric space.
pub const ARCFACE_TEMPLATE_112: [[f32; 2]; 5] = [
    [38.2946, 51.6963], // left eye
    [73.5318, 51.5014], // right eye
    [56.0252, 71.7366], // nose tip
    [41.5493, 92.3655], // left mouth corner
    [70.7299, 92.2041], // right mouth corner
];

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
        let face_112 = align_face_image(image, face);
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
/// When `face.score >= 0.3` the five detector keypoints are mapped to
/// [`ARCFACE_TEMPLATE_112`] via the Umeyama 2-D similarity transform, and the
/// result is sampled with bilinear interpolation.  For lower-confidence
/// detections the bbox-crop fallback (`align_and_crop`) is used instead.
///
/// # Errors
/// Returns [`CoreError::Image`] if `frame_rgb` does not match `width × height × 3`.
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
    Ok(align_face_image(&image, face))
}

/// Align a face from an already-decoded [`image::RgbImage`].
///
/// Uses Umeyama when `face.score >= 0.3`, bbox-crop otherwise.
pub(crate) fn align_face_image(image: &image::RgbImage, face: &DetectedFace) -> image::RgbImage {
    if face.score >= 0.3 {
        let m = umeyama_2d(&face.keypoints, &ARCFACE_TEMPLATE_112);
        warp_affine(image, m, 112, 112)
    } else {
        align_and_crop(image, face)
    }
}

/// Bbox-crop fallback: crop the face bounding box and resize to 112×112.
///
/// Used only when detection confidence is < 0.3.  Clamps coordinates to the
/// image boundary before cropping.
fn align_and_crop(image: &image::RgbImage, face: &DetectedFace) -> image::RgbImage {
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

// ─── Umeyama similarity transform ─────────────────────────────────────────────

/// Compute the Umeyama (1991) 2-D similarity transform mapping `src` landmarks
/// to `dst` template points.
///
/// Finds the optimal scale + rotation + translation (no reflection, 4 DOF) that
/// minimises the sum of squared distances between the mapped `src` points and
/// the `dst` points.
///
/// Returns a 2×3 affine matrix `[[a, b, tx], [c, d, ty]]` such that applying
/// `M * [x, y, 1]ᵀ` maps a source point into the template space.
fn umeyama_2d(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> [[f32; 3]; 2] {
    const N: f32 = 5.0;

    // 1. Centroids
    let mean_src = mean5(src);
    let mean_dst = mean5(dst);

    // 2. Centre both point sets
    let src_c: [[f32; 2]; 5] =
        std::array::from_fn(|i| [src[i][0] - mean_src[0], src[i][1] - mean_src[1]]);
    let dst_c: [[f32; 2]; 5] =
        std::array::from_fn(|i| [dst[i][0] - mean_dst[0], dst[i][1] - mean_dst[1]]);

    // 3. Source variance  var_src = (1/N) Σ ||src_cᵢ||²
    let var_src: f32 = src_c.iter().map(|p| p[0] * p[0] + p[1] * p[1]).sum::<f32>() / N;

    // 4. Cross-covariance  cov = (1/N) dst_cᵀ · src_c  (2×2 matrix)
    let mut cov = [[0_f32; 2]; 2];
    for i in 0..5 {
        cov[0][0] += dst_c[i][0] * src_c[i][0];
        cov[0][1] += dst_c[i][0] * src_c[i][1];
        cov[1][0] += dst_c[i][1] * src_c[i][0];
        cov[1][1] += dst_c[i][1] * src_c[i][1];
    }
    for row in &mut cov {
        for v in row.iter_mut() {
            *v /= N;
        }
    }

    // 5. SVD of 2×2 covariance (analytic, no external crate)
    let (u, s, vt, det_sign) = svd2x2(cov);

    // 6. Scale = trace(S·D) / var_src,  D = diag(1, det_sign)
    let scale = (s[0] + s[1] * det_sign) / var_src.max(1e-8);

    // 7. Rotation  R = U · D · Vᵀ
    let r = mat2x2_mul(mat2x2_mul(u, [[1.0, 0.0], [0.0, det_sign]]), vt);

    // 8. Translation  t = mean_dst − scale·R·mean_src
    let rmean = [
        r[0][0] * mean_src[0] + r[0][1] * mean_src[1],
        r[1][0] * mean_src[0] + r[1][1] * mean_src[1],
    ];
    let tx = mean_dst[0] - scale * rmean[0];
    let ty = mean_dst[1] - scale * rmean[1];

    [
        [scale * r[0][0], scale * r[0][1], tx],
        [scale * r[1][0], scale * r[1][1], ty],
    ]
}

/// Analytic SVD for a 2×2 matrix.
///
/// Returns `(U, [s1, s2], Vᵀ, det_sign)` where:
/// - `U` and `Vᵀ` are orthogonal (det = ±1),
/// - `s1 >= s2 >= 0` are singular values,
/// - `det_sign` is +1 if `det(U·Vᵀ) >= 0`, −1 otherwise (handles reflection).
fn svd2x2(m: [[f32; 2]; 2]) -> ([[f32; 2]; 2], [f32; 2], [[f32; 2]; 2], f32) {
    // Build AᵀA = Vᵀ·S²·V
    let ata = [
        [
            m[0][0] * m[0][0] + m[1][0] * m[1][0],
            m[0][0] * m[0][1] + m[1][0] * m[1][1],
        ],
        [
            m[0][1] * m[0][0] + m[1][1] * m[1][0],
            m[0][1] * m[0][1] + m[1][1] * m[1][1],
        ],
    ];

    // Eigenvalues of 2×2 symmetric AᵀA via the quadratic formula
    let trace = ata[0][0] + ata[1][1];
    let det = ata[0][0] * ata[1][1] - ata[0][1] * ata[1][0];
    let disc = ((trace * trace / 4.0 - det).max(0.0)).sqrt();
    let lam1 = (trace / 2.0 + disc).max(0.0);
    let lam2 = (trace / 2.0 - disc).max(0.0);
    let s1 = lam1.sqrt();
    let s2 = lam2.sqrt();

    // Eigenvectors of AᵀA → columns of V
    let v: [[f32; 2]; 2] = if ata[0][1].abs() < 1e-8 {
        // Already diagonal — V is identity
        [[1.0, 0.0], [0.0, 1.0]]
    } else {
        let v1 = normalize2d([ata[0][1], lam1 - ata[0][0]]);
        let v2 = [-v1[1], v1[0]]; // orthogonal complement
                                  // columns of V: V = [v1 | v2]
        [[v1[0], v2[0]], [v1[1], v2[1]]]
    };

    // U = A·V·S⁻¹  (for non-zero singular values)
    // First column of U
    let av0 = [
        m[0][0] * v[0][0] + m[0][1] * v[1][0],
        m[1][0] * v[0][0] + m[1][1] * v[1][0],
    ];
    let u0 = if s1 > 1e-8 {
        normalize2d(av0)
    } else {
        [1.0, 0.0]
    };
    // Second column of U (orthogonal complement)
    let u1 = [-u0[1], u0[0]];

    // U stored column-major: U = [u0 | u1]
    let u = [[u0[0], u1[0]], [u0[1], u1[1]]];
    // Vᵀ = transpose of V
    let vt = [[v[0][0], v[1][0]], [v[0][1], v[1][1]]];

    // det_sign: sign of det(U·Vᵀ)
    let uvt = mat2x2_mul(u, vt);
    let det_uvt = uvt[0][0] * uvt[1][1] - uvt[0][1] * uvt[1][0];
    let det_sign = if det_uvt >= 0.0 { 1.0_f32 } else { -1.0_f32 };

    (u, [s1, s2], vt, det_sign)
}

/// Apply a 2×3 affine matrix to warp `src` into an `out_w × out_h` output.
///
/// Uses inverse mapping: for each output pixel `(dx, dy)` the source
/// coordinates are computed via the inverse of `m` and the value is sampled
/// with bilinear interpolation.  Out-of-bounds source pixels are filled black.
fn warp_affine(src: &image::RgbImage, m: [[f32; 3]; 2], out_w: u32, out_h: u32) -> image::RgbImage {
    let (src_w, src_h) = (src.width() as f32, src.height() as f32);
    let mut dst = image::RgbImage::new(out_w, out_h);

    // Invert the 2×3 affine matrix
    // M = [[a, b, tx], [c, d, ty]]  det = a·d − b·c
    let a = m[0][0];
    let b = m[0][1];
    let tx = m[0][2];
    let c = m[1][0];
    let d = m[1][1];
    let ty = m[1][2];
    let det = a * d - b * c;
    if det.abs() < 1e-8 {
        // Degenerate transform — return black image
        return dst;
    }
    let inv_det = 1.0 / det;
    // Inverse: [[d, -b, b·ty − d·tx], [-c, a, c·tx − a·ty]] * inv_det
    let inv = [
        [d * inv_det, -b * inv_det, (b * ty - d * tx) * inv_det],
        [-c * inv_det, a * inv_det, (c * tx - a * ty) * inv_det],
    ];

    for dy in 0..out_h {
        for dx in 0..out_w {
            // Map output pixel back to source space
            let sx = inv[0][0] * dx as f32 + inv[0][1] * dy as f32 + inv[0][2];
            let sy = inv[1][0] * dx as f32 + inv[1][1] * dy as f32 + inv[1][2];

            if sx < 0.0 || sy < 0.0 || sx >= src_w - 1.0 || sy >= src_h - 1.0 {
                dst.put_pixel(dx, dy, image::Rgb([0, 0, 0]));
                continue;
            }

            // Bilinear interpolation
            let x0 = sx.floor() as u32;
            let y0 = sy.floor() as u32;
            let x1 = (x0 + 1).min(src.width() - 1);
            let y1 = (y0 + 1).min(src.height() - 1);
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let p00 = src.get_pixel(x0, y0).0;
            let p10 = src.get_pixel(x1, y0).0;
            let p01 = src.get_pixel(x0, y1).0;
            let p11 = src.get_pixel(x1, y1).0;

            let interp = |ch: usize| -> u8 {
                let top = p00[ch] as f32 * (1.0 - fx) + p10[ch] as f32 * fx;
                let bot = p01[ch] as f32 * (1.0 - fx) + p11[ch] as f32 * fx;
                (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8
            };

            dst.put_pixel(dx, dy, image::Rgb([interp(0), interp(1), interp(2)]));
        }
    }
    dst
}

// ─── Math helpers ──────────────────────────────────────────────────────────────

/// Compute the centroid of five 2-D points.
fn mean5(pts: &[[f32; 2]; 5]) -> [f32; 2] {
    let sx: f32 = pts.iter().map(|p| p[0]).sum();
    let sy: f32 = pts.iter().map(|p| p[1]).sum();
    [sx / 5.0, sy / 5.0]
}

/// Return a unit-length vector in the same direction as `v`.
///
/// Returns `[1.0, 0.0]` if the magnitude is below `1e-8` (degenerate input).
fn normalize2d(v: [f32; 2]) -> [f32; 2] {
    let n = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if n < 1e-8 {
        return [1.0, 0.0];
    }
    [v[0] / n, v[1] / n]
}

/// Multiply two 2×2 matrices.
fn mat2x2_mul(a: [[f32; 2]; 2], b: [[f32; 2]; 2]) -> [[f32; 2]; 2] {
    [
        [
            a[0][0] * b[0][0] + a[0][1] * b[1][0],
            a[0][0] * b[0][1] + a[0][1] * b[1][1],
        ],
        [
            a[1][0] * b[0][0] + a[1][1] * b[1][0],
            a[1][0] * b[0][1] + a[1][1] * b[1][1],
        ],
    ]
}

// ─── Preprocessing ─────────────────────────────────────────────────────────────

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

    // ─── Umeyama / alignment tests ──────────────────────────────────────────

    /// Identity transform: when src == dst the matrix should be ≈ [[1,0,0],[0,1,0]].
    #[test]
    fn test_umeyama_identity_transform() {
        let pts = ARCFACE_TEMPLATE_112;
        let m = umeyama_2d(&pts, &pts);
        assert!(
            (m[0][0] - 1.0).abs() < 0.01,
            "a (scale·cos) should ≈ 1.0, got {}",
            m[0][0]
        );
        assert!(
            (m[1][1] - 1.0).abs() < 0.01,
            "d (scale·cos) should ≈ 1.0, got {}",
            m[1][1]
        );
        assert!(
            m[0][1].abs() < 0.01,
            "b (scale·sin) should ≈ 0, got {}",
            m[0][1]
        );
        assert!(m[0][2].abs() < 0.5, "tx should ≈ 0, got {}", m[0][2]);
        assert!(m[1][2].abs() < 0.5, "ty should ≈ 0, got {}", m[1][2]);
    }

    /// Warp with identity matrix must produce a 112×112 output.
    #[test]
    fn test_warp_affine_identity_produces_correct_size() {
        let img = image::RgbImage::new(200, 200);
        let identity = [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let result = warp_affine(&img, identity, 112, 112);
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
    }

    /// Warp with identity on a 112×112 image must copy pixels faithfully.
    #[test]
    fn test_warp_affine_identity() {
        let img = image::RgbImage::from_pixel(112, 112, image::Rgb([80, 120, 200]));
        let identity = [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let result = warp_affine(&img, identity, 112, 112);
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
        // Interior pixels should be copied unchanged (boundary pixels may be
        // out-of-range due to the sx < src_w - 1 guard, so we sample centre).
        let centre = result.get_pixel(56, 56).0;
        assert_eq!(centre, [80, 120, 200], "identity warp should copy pixels");
    }

    /// Low detection score → bbox fallback path, still 112×112.
    #[test]
    fn test_align_face_fallback_low_score() {
        let img = image::RgbImage::new(200, 200);
        let face = DetectedFace {
            bbox: [10.0, 10.0, 110.0, 110.0],
            keypoints: [[0.0; 2]; 5],
            score: 0.2, // below 0.3 → bbox fallback
        };
        let result = align_face_image(&img, &face);
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
    }

    /// High detection score → Umeyama path, output still 112×112.
    #[test]
    fn test_align_face_high_score_uses_umeyama() {
        let img = image::RgbImage::from_pixel(200, 200, image::Rgb([128, 128, 128]));
        // Approximate ArcFace template coordinates scaled to a 200-pixel image
        let face = DetectedFace {
            bbox: [10.0, 10.0, 190.0, 190.0],
            keypoints: [
                [68.0, 92.0],   // left eye
                [131.0, 91.0],  // right eye
                [100.0, 128.0], // nose
                [74.0, 164.0],  // left mouth
                [126.0, 164.0], // right mouth
            ],
            score: 0.95,
        };
        let result = align_face_image(&img, &face);
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
    }

    /// `align_face` (public entry point) must also yield 112×112.
    #[test]
    fn test_umeyama_output_dimensions() {
        let frame = vec![128u8; 200 * 200 * 3];
        let face = DetectedFace {
            bbox: [10.0, 10.0, 190.0, 190.0],
            keypoints: [
                [68.0, 92.0],
                [131.0, 91.0],
                [100.0, 128.0],
                [74.0, 164.0],
                [126.0, 164.0],
            ],
            score: 0.95,
        };
        let result = align_face(&frame, 200, 200, &face).unwrap();
        assert_eq!(result.width(), 112);
        assert_eq!(result.height(), 112);
    }

    /// 45° rotation of the template should produce a rotation matrix with
    /// off-diagonal elements ≈ ±sin(45°) ≈ ±0.707.
    #[test]
    fn test_umeyama_2d_rotation_only() {
        // Rotate the template points by 45° around their centroid
        let angle: f32 = std::f32::consts::FRAC_PI_4; // 45°
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        let centre = mean5(&ARCFACE_TEMPLATE_112);
        let rotated: [[f32; 2]; 5] = std::array::from_fn(|i| {
            let dx = ARCFACE_TEMPLATE_112[i][0] - centre[0];
            let dy = ARCFACE_TEMPLATE_112[i][1] - centre[1];
            [
                centre[0] + cos_a * dx - sin_a * dy,
                centre[1] + sin_a * dx + cos_a * dy,
            ]
        });
        // umeyama_2d(rotated → template) should recover a −45° rotation with scale ≈ 1
        let m = umeyama_2d(&rotated, &ARCFACE_TEMPLATE_112);
        let scale = (m[0][0] * m[0][0] + m[1][0] * m[1][0]).sqrt();
        assert!(
            (scale - 1.0).abs() < 0.02,
            "scale should ≈ 1.0 for pure rotation, got {scale}"
        );
        // The recovered rotation angle should be ≈ −45°  (or +315°)
        let recovered_angle = m[1][0].atan2(m[0][0]); // atan2(c, a)
        let diff = (recovered_angle - (-angle)).abs();
        let diff = diff.min(std::f32::consts::TAU - diff);
        assert!(
            diff < 0.05,
            "rotation angle should ≈ −45°, got {recovered_angle:.4} rad (diff {diff:.4})"
        );
    }

    /// U and Vᵀ from svd2x2 must be orthogonal (det ≈ ±1).
    #[test]
    fn test_svd2x2_orthogonal_result() {
        let m = [[3.0_f32, 1.0], [1.0, 2.0]];
        let (u, _s, vt, _) = svd2x2(m);
        let det_u = u[0][0] * u[1][1] - u[0][1] * u[1][0];
        let det_vt = vt[0][0] * vt[1][1] - vt[0][1] * vt[1][0];
        assert!(
            (det_u.abs() - 1.0).abs() < 0.01,
            "det(U) should be ±1, got {det_u}"
        );
        assert!(
            (det_vt.abs() - 1.0).abs() < 0.01,
            "det(Vᵀ) should be ±1, got {det_vt}"
        );
    }

    /// For a diagonal matrix [[3,0],[0,1]], SVD should return S=[3,1], U=I, Vᵀ=I.
    #[test]
    fn test_svd2x2_diagonal_matrix() {
        let m = [[3.0_f32, 0.0], [0.0, 1.0]];
        let (u, s, vt, _) = svd2x2(m);
        assert!((s[0] - 3.0).abs() < 0.01, "s1 should be 3.0, got {}", s[0]);
        assert!((s[1] - 1.0).abs() < 0.01, "s2 should be 1.0, got {}", s[1]);
        // U and Vᵀ should both be close to identity (or both negated consistently)
        let det_u = u[0][0] * u[1][1] - u[0][1] * u[1][0];
        let det_vt = vt[0][0] * vt[1][1] - vt[0][1] * vt[1][0];
        assert!(
            (det_u.abs() - 1.0).abs() < 0.01,
            "det(U) should be ±1, got {det_u}"
        );
        assert!(
            (det_vt.abs() - 1.0).abs() < 0.01,
            "det(Vᵀ) should be ±1, got {det_vt}"
        );
        // Reconstruction A ≈ U · diag(s) · Vᵀ
        let recon = [
            [
                u[0][0] * s[0] * vt[0][0] + u[0][1] * s[1] * vt[1][0],
                u[0][0] * s[0] * vt[0][1] + u[0][1] * s[1] * vt[1][1],
            ],
            [
                u[1][0] * s[0] * vt[0][0] + u[1][1] * s[1] * vt[1][0],
                u[1][0] * s[0] * vt[0][1] + u[1][1] * s[1] * vt[1][1],
            ],
        ];
        assert!((recon[0][0] - 3.0).abs() < 0.01, "recon[0][0] ≈ 3");
        assert!(recon[0][1].abs() < 0.01, "recon[0][1] ≈ 0");
        assert!(recon[1][0].abs() < 0.01, "recon[1][0] ≈ 0");
        assert!((recon[1][1] - 1.0).abs() < 0.01, "recon[1][1] ≈ 1");
    }
}
