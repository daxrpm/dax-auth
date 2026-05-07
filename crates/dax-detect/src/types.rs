/// 2D point in image-space (pixel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2D {
    pub x: f32,
    pub y: f32,
}

/// Axis-aligned bounding box in image-space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl Bbox {
    #[must_use]
    pub fn width(&self) -> f32 {
        (self.x2 - self.x1).max(0.0)
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        (self.y2 - self.y1).max(0.0)
    }

    #[must_use]
    pub fn area(&self) -> f32 {
        self.width() * self.height()
    }

    /// Intersection-over-union with another box. Returns 0.0 when the
    /// boxes do not overlap.
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn iou(&self, other: &Self) -> f32 {
        let inter_x1 = self.x1.max(other.x1);
        let inter_y1 = self.y1.max(other.y1);
        let inter_x2 = self.x2.min(other.x2);
        let inter_y2 = self.y2.min(other.y2);

        let inter_w = (inter_x2 - inter_x1).max(0.0);
        let inter_h = (inter_y2 - inter_y1).max(0.0);
        let inter = inter_w * inter_h;

        let union = self.area() + other.area() - inter;
        if union <= 0.0 {
            0.0
        } else {
            inter / union
        }
    }
}

/// The five canonical face landmarks emitted by SCRFD.
///
/// Used downstream to perform an affine warp that aligns the face
/// before computing the recognition embedding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landmarks {
    pub left_eye: Point2D,
    pub right_eye: Point2D,
    pub nose: Point2D,
    pub left_mouth: Point2D,
    pub right_mouth: Point2D,
}

/// A single detection emitted by the face detector.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceDetection {
    pub bbox: Bbox,
    pub score: f32,
    pub landmarks: Landmarks,
}
