//! Camera enumeration and frame capture.
//!
//! The public surface is intentionally small: callers describe what
//! they want (an enumerator or a camera handle) and let the backend
//! pick the platform-specific path. The current implementation is
//! V4L2-only via `nokhwa`; future backends (`libcamera`, `RealSense`)
//! will plug in behind the same trait.

mod enumerator;
mod error;

pub use enumerator::{DeviceInfo, Enumerator};
pub use error::{CaptureError, CaptureResult};
