//! # dax-auth-camera
//!
//! Camera abstraction layer for dax-auth.
//!
//! Provides a unified interface over:
//! - Standard RGB webcams (V4L2)
//! - IR cameras (V4L2 with IR pixel format detection)
//! - Depth cameras (future: librealsense2)
//!
//! ## Camera type detection
//! On open, we probe the device capabilities to determine if it supports
//! infrared formats (`Y16`, `GREY`, `Y800`) which enables true liveness detection.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![warn(clippy::pedantic)]

pub mod capture;
pub mod device;
pub mod error;
pub mod frame;

pub use capture::CameraCapture;
pub use device::{CameraDevice, CameraKind};
pub use error::CameraError;
pub use frame::Frame;
