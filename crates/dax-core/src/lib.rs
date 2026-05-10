//! Cross-cutting domain types for the dax-auth project.
//!
//! Subdomains (capture, detection, embedding, etc.) own their own
//! error types and logic. This crate hosts only the data shapes that
//! must be shared between them.

pub mod frame;

pub use frame::{Frame, PixelFormat};
