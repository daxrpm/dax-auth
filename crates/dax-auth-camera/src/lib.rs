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
//!
//! ## Unsafe code
//! Unsafe code is permitted only in [`capture`] where a single `transmute` is
//! used to erase a phantom lifetime on `v4l::io::mmap::Stream<'a>`.  The
//! `Arena` buffers that carry that lifetime are mmap-backed kernel pages; they
//! do not borrow from the `v4l::Device` in any way.  Soundness is maintained
//! by storing both the `Device` and the `Stream` inside the same
//! `CameraCapture` struct, guaranteeing the fd outlives the stream.

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
