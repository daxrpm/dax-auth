//! Camera enumeration and frame capture.
//!
//! The public surface is intentionally small: callers describe what
//! they want (an enumerator or a camera handle) and let the backend
//! pick the platform-specific path. The current implementation is
//! V4L2-only via `nokhwa`; future backends (`libcamera`, `RealSense`)
//! will plug in behind the same trait.

mod camera;
mod enumerator;
mod error;

pub use camera::Camera;
pub use enumerator::{DeviceInfo, Enumerator};
pub use error::{CaptureError, CaptureResult};

// Re-export the cross-crate frame type so consumers do not need to
// depend on `dax-core` directly.
pub use dax_core::{Frame, PixelFormat};
