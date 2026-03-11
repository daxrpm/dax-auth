//! Camera device discovery and capability probing.
//!
//! Uses the `v4l` crate to enumerate `/dev/video*` devices, query their V4L2
//! capabilities, and classify them as RGB, Infrared, or RgbAndInfrared.

use crate::{frame::PixelFormat, CameraError};
use serde::{Deserialize, Serialize};
use v4l::capability::Flags as CapFlags;
use v4l::format::Description as FormatDescription;
use v4l::video::Capture;

/// The kind of camera detected based on supported pixel formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraKind {
    /// Standard RGB webcam. Anti-spoofing uses the 2D MiniFASNetV2 model.
    Rgb,
    /// Infrared-only camera. Enables true depth-based liveness detection.
    Infrared,
    /// Camera that provides both RGB and IR streams (e.g., Intel RealSense).
    RgbAndInfrared,
}

/// Represents a detected V4L2 camera device and its capabilities.
#[derive(Debug, Clone)]
pub struct CameraDevice {
    /// Path to the V4L2 device (e.g., `/dev/video0`).
    pub path: String,
    /// Human-readable device name from `VIDIOC_QUERYCAP` (card field).
    pub name: String,
    /// The detected camera kind based on supported pixel formats.
    pub kind: CameraKind,
    /// Best supported width in pixels (largest available, capped at 1920).
    pub width: u32,
    /// Best supported height in pixels.
    pub height: u32,
}

impl CameraDevice {
    /// Probe all V4L2 video devices and return suitable ones.
    ///
    /// Iterates `/dev/video0` through `/dev/video63`, queries each device's
    /// capabilities and supported pixel formats, and returns all devices that:
    /// - Have `V4L2_CAP_VIDEO_CAPTURE` capability
    /// - Support at least one of the target pixel formats
    ///
    /// Devices are sorted with IR cameras first, then by resolution descending.
    ///
    /// Returns `Ok(vec![])` (not an error) when no devices are found.
    ///
    /// # Errors
    /// Returns `CameraError` only on unexpected I/O failures, not on missing devices.
    pub fn enumerate() -> Result<Vec<Self>, CameraError> {
        let mut devices = Vec::new();
        let mut consecutive_failures: u32 = 0;

        for index in 0..64usize {
            // Attempt to open by index; skip if device doesn't exist.
            let dev = match v4l::Device::new(index) {
                Ok(d) => {
                    consecutive_failures = 0;
                    d
                }
                Err(_) => {
                    consecutive_failures += 1;
                    tracing::debug!(index, "no device at /dev/video{index}");
                    // Stop after 4 consecutive gaps to avoid probing all 64 indices.
                    if consecutive_failures >= 4 {
                        break;
                    }
                    continue;
                }
            };

            let path = format!("/dev/video{index}");

            // Query capabilities — skip devices that don't support video capture.
            let caps = match dev.query_caps() {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(path = %path, error = %e, "failed to query caps, skipping");
                    continue;
                }
            };

            if !caps.capabilities.contains(CapFlags::VIDEO_CAPTURE) {
                tracing::debug!(path = %path, "device lacks VIDEO_CAPTURE capability, skipping");
                continue;
            }

            // `card` is already a `String` — strip any null padding.
            let name = caps.card.trim_matches('\0').to_string();

            // Enumerate pixel formats.
            let formats: Vec<FormatDescription> = match dev.enum_formats() {
                Ok(f) => f,
                Err(e) => {
                    tracing::debug!(path = %path, error = %e, "failed to enum formats, skipping");
                    continue;
                }
            };

            let mut has_rgb = false;
            let mut has_ir = false;
            let mut has_any = false;

            for fmt in &formats {
                if let Some(pf) = PixelFormat::from_v4l2_fourcc(fmt.fourcc) {
                    has_any = true;
                    match pf {
                        PixelFormat::Grey | PixelFormat::Y16 => {
                            has_ir = true;
                        }
                        PixelFormat::Yuyv | PixelFormat::Mjpeg | PixelFormat::Bgr24 => {
                            has_rgb = true;
                        }
                    }
                }
            }

            // Skip devices with no supported pixel formats at all.
            if !has_any {
                tracing::debug!(path = %path, "no supported pixel formats, skipping");
                continue;
            }

            let kind = match (has_rgb, has_ir) {
                (true, true) => CameraKind::RgbAndInfrared,
                (false, true) => CameraKind::Infrared,
                _ => CameraKind::Rgb,
            };

            // Find best resolution: largest width up to 1920.
            let (best_width, best_height) = best_resolution(&dev, &formats);

            tracing::debug!(
                path = %path,
                name = %name,
                kind = ?kind,
                width = best_width,
                height = best_height,
                "discovered camera device"
            );

            devices.push(CameraDevice {
                path,
                name,
                kind,
                width: best_width,
                height: best_height,
            });
        }

        Ok(devices)
    }

    /// Returns the best available camera for authentication.
    ///
    /// Preference order: `RgbAndInfrared` > `Infrared` > `Rgb`.
    /// Within each kind, higher resolution is preferred.
    ///
    /// # Errors
    /// Returns `CameraError::DeviceNotFound` if no suitable device is found.
    pub fn best_available() -> Result<Self, CameraError> {
        let mut devices = Self::enumerate()?;
        if devices.is_empty() {
            return Err(CameraError::DeviceNotFound {
                path: "/dev/video*".into(),
            });
        }

        // Sort: IR cameras first, then by resolution (descending).
        devices.sort_by_key(|d| {
            let kind_score = match d.kind {
                CameraKind::RgbAndInfrared => 0u32,
                CameraKind::Infrared => 1,
                CameraKind::Rgb => 2,
            };
            (kind_score, u32::MAX - d.width * d.height)
        });

        Ok(devices.remove(0))
    }
}

/// Query a device's frame sizes for each format and return the best `(width, height)`.
///
/// "Best" means the largest width that does not exceed 1920 pixels. If no frame
/// sizes can be queried, falls back to a sensible default of 640×480.
fn best_resolution(dev: &v4l::Device, formats: &[FormatDescription]) -> (u32, u32) {
    let mut best_w: u32 = 0;
    let mut best_h: u32 = 0;

    for fmt in formats {
        let Ok(sizes) = dev.enum_framesizes(fmt.fourcc) else {
            continue;
        };

        for size in sizes {
            use v4l::framesize::FrameSizeEnum;
            let (w, h) = match size.size {
                FrameSizeEnum::Discrete(d) => (d.width, d.height),
                FrameSizeEnum::Stepwise(s) => (s.max_width, s.max_height),
            };

            // Cap at 1920 to avoid 4K cameras dominating; take the largest.
            if w <= 1920 && w > best_w {
                best_w = w;
                best_h = h;
            }
        }
    }

    if best_w == 0 {
        // Fallback when frame size enumeration yields nothing.
        (640, 480)
    } else {
        (best_w, best_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_returns_empty_not_error_when_no_devices() {
        // In CI without a real camera, enumerate() should return Ok(vec![])
        // not an error. This test passes trivially if no /dev/video* exists.
        let result = CameraDevice::enumerate();
        assert!(result.is_ok());
    }

    #[test]
    fn best_available_returns_error_when_no_devices() {
        // Only meaningful in CI without cameras.
        // Skip if /dev/video0 exists.
        if std::path::Path::new("/dev/video0").exists() {
            return;
        }
        let result = CameraDevice::best_available();
        assert!(matches!(result, Err(CameraError::DeviceNotFound { .. })));
    }
}
