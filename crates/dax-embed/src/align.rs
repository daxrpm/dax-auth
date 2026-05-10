// Mean / variance / coordinate maths use deliberate `usize → f32`
// casts that cannot in practice exceed the f32 mantissa.
#![allow(clippy::cast_precision_loss)]

use dax_detect::{Landmarks, Point2D};
use nalgebra::{Matrix2, Vector2};
use tracing::trace;

use crate::error::{EmbedError, EmbedResult};

/// Side length of the aligned face canvas expected by `ArcFace` /
/// `MobileFaceNet` trained at 112×112.
pub const ALIGNED_SIZE: u32 = 112;

/// Canonical destination landmarks for the `ArcFace` 112×112 input,
/// taken from the reference `InsightFace` implementation.
const CANONICAL: [Point2D; 5] = [
    Point2D {
        x: 38.2946,
        y: 51.6963,
    },
    Point2D {
        x: 73.5318,
        y: 51.5014,
    },
    Point2D {
        x: 56.0252,
        y: 71.7366,
    },
    Point2D {
        x: 41.5493,
        y: 92.3655,
    },
    Point2D {
        x: 70.7299,
        y: 92.2041,
    },
];

/// 2D affine transform expressed as `y = A·x + t`.
#[derive(Debug, Clone, Copy)]
pub struct AffineTransform {
    pub a: Matrix2<f32>,
    pub t: Vector2<f32>,
}

impl AffineTransform {
    #[must_use]
    pub fn invert(&self) -> Option<Self> {
        let inv_a = self.a.try_inverse()?;
        let inv_t = -inv_a * self.t;
        Some(Self { a: inv_a, t: inv_t })
    }
}

/// Estimate a least-squares similarity transform that maps the input
/// landmarks onto the canonical `ArcFace` targets.
///
/// Implements the `Umeyama` (1991) closed-form solution restricted to
/// 2D, with reflection handling driven by `det(A)`.
pub fn estimate_alignment(landmarks: &Landmarks) -> EmbedResult<AffineTransform> {
    let src: [Point2D; 5] = [
        landmarks.left_eye,
        landmarks.right_eye,
        landmarks.nose,
        landmarks.left_mouth,
        landmarks.right_mouth,
    ];
    let dst = CANONICAL;

    let n = src.len() as f32;
    let src_mean = mean(&src);
    let dst_mean = mean(&dst);

    let mut a = Matrix2::<f32>::zeros();
    let mut var_src = 0.0_f32;
    for i in 0..src.len() {
        let s = Vector2::new(src[i].x - src_mean.x, src[i].y - src_mean.y);
        let d = Vector2::new(dst[i].x - dst_mean.x, dst[i].y - dst_mean.y);
        a += d * s.transpose();
        var_src += s.norm_squared();
    }
    a /= n;
    var_src /= n;

    if var_src <= f32::EPSILON {
        return Err(EmbedError::Alignment(String::from(
            "input landmarks are degenerate (zero variance)",
        )));
    }

    let svd = a.svd(true, true);
    let u = svd
        .u
        .ok_or_else(|| EmbedError::Alignment(String::from("SVD failed (U missing)")))?;
    let v_t = svd
        .v_t
        .ok_or_else(|| EmbedError::Alignment(String::from("SVD failed (V^T missing)")))?;

    // Reflection handling: ensure rotation has determinant +1.
    let mut d_diag = Vector2::new(1.0_f32, 1.0_f32);
    if a.determinant() < 0.0 {
        d_diag.y = -1.0;
    }
    let d_mat = Matrix2::new(d_diag.x, 0.0, 0.0, d_diag.y);

    let rotation = u * d_mat * v_t;
    let scale = svd.singular_values.dot(&d_diag) / var_src;
    let scaled_rot = rotation * scale;

    let src_mean_v = Vector2::new(src_mean.x, src_mean.y);
    let dst_mean_v = Vector2::new(dst_mean.x, dst_mean.y);
    let intermediate = scaled_rot * src_mean_v;
    let translation = dst_mean_v - intermediate;
    trace!(
        ?src_mean_v,
        ?dst_mean_v,
        rotation = ?rotation,
        scale,
        ?scaled_rot,
        product = ?intermediate,
        ?translation,
        "umeyama intermediate"
    );

    Ok(AffineTransform {
        a: scaled_rot,
        t: translation,
    })
}

fn mean(points: &[Point2D]) -> Point2D {
    let n = points.len() as f32;
    let mut acc = Point2D { x: 0.0, y: 0.0 };
    for p in points {
        acc.x += p.x;
        acc.y += p.y;
    }
    Point2D {
        x: acc.x / n,
        y: acc.y / n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lm(eyes: [(f32, f32); 2], nose: (f32, f32), mouth: [(f32, f32); 2]) -> Landmarks {
        Landmarks {
            left_eye: Point2D {
                x: eyes[0].0,
                y: eyes[0].1,
            },
            right_eye: Point2D {
                x: eyes[1].0,
                y: eyes[1].1,
            },
            nose: Point2D {
                x: nose.0,
                y: nose.1,
            },
            left_mouth: Point2D {
                x: mouth[0].0,
                y: mouth[0].1,
            },
            right_mouth: Point2D {
                x: mouth[1].0,
                y: mouth[1].1,
            },
        }
    }

    #[test]
    fn identity_when_landmarks_match_canonical() {
        // Feeding the canonical landmarks back must produce identity.
        let l = lm(
            [(38.2946, 51.6963), (73.5318, 51.5014)],
            (56.0252, 71.7366),
            [(41.5493, 92.3655), (70.7299, 92.2041)],
        );
        let t = estimate_alignment(&l).unwrap();
        assert!((t.a[(0, 0)] - 1.0).abs() < 1e-3);
        assert!((t.a[(1, 1)] - 1.0).abs() < 1e-3);
        assert!(t.a[(0, 1)].abs() < 1e-3);
        assert!(t.a[(1, 0)].abs() < 1e-3);
        assert!(t.t.norm() < 1e-3);
    }
}
