//! # dax-auth-core
//!
//! ML inference engine for dax-auth facial authentication.
//!
//! ## Pipeline
//! ```text
//! Frame → Detection → Alignment → [Liveness] → Recognition → Matching
//! ```
//!
//! ## Execution Providers (ONNX Runtime)
//! Priority order at runtime:
//! 1. ROCm (AMD GPU) — feature `rocm`
//! 2. CUDA (NVIDIA GPU) — feature `cuda`
//! 3. OpenVINO (Intel) — feature `openvino`
//! 4. CPU — always available (fallback)
//!
//! VitisAI (AMD Ryzen AI NPU) is architecturally ready but
//! deferred until the VitisAI EP is stable in the `ort` crate.
//! Enable by adding `vitisai` feature when available.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![warn(clippy::pedantic)]

pub mod config;
pub mod detection;
pub mod embedding;
pub mod error;
pub mod liveness;
pub mod models;
pub mod pipeline;
pub mod store;

pub use config::CoreConfig;
pub use error::CoreError;
pub use pipeline::{AuthPipeline, FailureStage, PipelineResult};
