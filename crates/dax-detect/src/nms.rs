use crate::types::FaceDetection;

/// Greedy non-maximum suppression by score.
///
/// Detections are sorted descending by score, then iterated; a
/// detection is kept iff its `IoU` with every previously-kept box is
/// below `iou_threshold`. The classic O(N²) variant — N is small
/// after the score filter, so a heap is overkill.
pub fn non_maximum_suppression(
    mut detections: Vec<FaceDetection>,
    iou_threshold: f32,
) -> Vec<FaceDetection> {
    detections.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut kept: Vec<FaceDetection> = Vec::with_capacity(detections.len());
    for det in detections {
        let overlaps = kept.iter().any(|k| det.bbox.iou(&k.bbox) > iou_threshold);
        if !overlaps {
            kept.push(det);
        }
    }
    kept
}
