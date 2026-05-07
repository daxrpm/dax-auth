// Decoder arithmetic uses lossy numeric casts that mirror the
// reference InsightFace implementation; clippy's pedantic warnings
// for that family are noise here.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use ndarray::ArrayView2;

use crate::preprocess::{LetterboxGeometry, INPUT_SIZE};
use crate::types::{Bbox, FaceDetection, Landmarks, Point2D};

/// Stride / output-name triples that describe `SCRFD-500MF`'s heads.
///
/// (`stride`, `scores`, `bbox_offsets`, `keypoint_offsets`).
pub const SCRFD_HEADS: [StrideHead; 3] = [
    StrideHead {
        stride: 8,
        scores: "443",
        bbox: "446",
        kps: "449",
    },
    StrideHead {
        stride: 16,
        scores: "468",
        bbox: "471",
        kps: "474",
    },
    StrideHead {
        stride: 32,
        scores: "493",
        bbox: "496",
        kps: "499",
    },
];

/// SCRFD-500MF emits two anchors per grid cell.
pub const ANCHORS_PER_CELL: usize = 2;

/// Anchors with a score below this are discarded before NMS.
pub const SCORE_THRESHOLD: f32 = 0.5;

/// `IoU` above this triggers suppression in NMS.
pub const IOU_THRESHOLD: f32 = 0.4;

/// Number of (x, y) keypoints per face.
const KEYPOINTS_PER_FACE: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct StrideHead {
    pub stride: u32,
    pub scores: &'static str,
    pub bbox: &'static str,
    pub kps: &'static str,
}

/// Decode every anchor of a single stride into pre-NMS detections in
/// **source-image** coordinates.
///
/// Views are passed by value because `ArrayView2` is itself a fat
/// pointer — taking it by reference would only add indirection.
#[allow(clippy::needless_pass_by_value)]
pub fn decode_stride(
    head: StrideHead,
    scores: ArrayView2<'_, f32>,
    bbox: ArrayView2<'_, f32>,
    kps: ArrayView2<'_, f32>,
    geometry: LetterboxGeometry,
    score_threshold: f32,
) -> Vec<FaceDetection> {
    let stride_f = head.stride as f32;
    let grid_size = INPUT_SIZE / head.stride;
    let total_rows = (grid_size as usize) * (grid_size as usize) * ANCHORS_PER_CELL;
    debug_assert_eq!(scores.shape()[0], total_rows);

    let inv_scale = 1.0 / geometry.scale;
    let pad_x = geometry.pad_x as f32;
    let pad_y = geometry.pad_y as f32;
    let to_src_x = |x: f32| (x - pad_x) * inv_scale;
    let to_src_y = |y: f32| (y - pad_y) * inv_scale;

    let mut out = Vec::new();
    for row in 0..total_rows {
        let score = scores[[row, 0]];
        if score < score_threshold {
            continue;
        }

        let cell = row / ANCHORS_PER_CELL;
        let cell_x = (cell as u32) % grid_size;
        let cell_y = (cell as u32) / grid_size;
        let anchor_x = cell_x as f32 * stride_f;
        let anchor_y = cell_y as f32 * stride_f;

        let dl = bbox[[row, 0]] * stride_f;
        let dt = bbox[[row, 1]] * stride_f;
        let dr = bbox[[row, 2]] * stride_f;
        let db = bbox[[row, 3]] * stride_f;

        let bbox_out = Bbox {
            x1: to_src_x(anchor_x - dl),
            y1: to_src_y(anchor_y - dt),
            x2: to_src_x(anchor_x + dr),
            y2: to_src_y(anchor_y + db),
        };

        let mut points = [Point2D { x: 0.0, y: 0.0 }; KEYPOINTS_PER_FACE];
        for (i, point) in points.iter_mut().enumerate() {
            let dx = kps[[row, i * 2]] * stride_f;
            let dy = kps[[row, i * 2 + 1]] * stride_f;
            point.x = to_src_x(anchor_x + dx);
            point.y = to_src_y(anchor_y + dy);
        }

        out.push(FaceDetection {
            bbox: bbox_out,
            score,
            landmarks: Landmarks {
                left_eye: points[0],
                right_eye: points[1],
                nose: points[2],
                left_mouth: points[3],
                right_mouth: points[4],
            },
        });
    }
    out
}
