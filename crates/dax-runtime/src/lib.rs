//! End-to-end face authentication pipeline.
//!
//! Both the CLI's `verify` subcommand and the PAM module call into
//! [`verify_face`]. Keeping the pipeline in a dedicated crate avoids
//! drift between the two entry points and lets the heavyweight model
//! initialisation be unit-tested independently.

mod config;
mod error;
mod verify;

pub use config::{CameraConfig, Config, PathsConfig, SecurityConfig, SYSTEM_CONFIG_PATH};
pub use error::{RuntimeError, RuntimeResult};
pub use verify::{
    verify_face, IrCheckOutcome, VerifyConfig, VerifyOutcome, VerifyReason,
    DEFAULT_IR_CENTER_TOLERANCE, DEFAULT_MATCH_THRESHOLD,
};
