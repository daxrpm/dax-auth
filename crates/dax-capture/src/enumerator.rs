use nokhwa::{nokhwa_initialize, query, utils::ApiBackend};
use tracing::debug;

use crate::error::{CaptureError, CaptureResult};

/// Lightweight description of a camera device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub path: Option<String>,
}

/// Discovers cameras available to the operating system.
#[derive(Debug, Default)]
pub struct Enumerator;

impl Enumerator {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// List every camera the platform exposes.
    ///
    /// On Linux this maps to `/dev/video*` entries handled by V4L2.
    /// The backend is initialised on first call; subsequent calls
    /// are cheap.
    pub fn list(&self) -> CaptureResult<Vec<DeviceInfo>> {
        ensure_backend_ready();

        let cameras =
            query(ApiBackend::Auto).map_err(|e| CaptureError::Enumerate(e.to_string()))?;

        debug!(count = cameras.len(), "enumerated cameras");

        let devices = cameras
            .into_iter()
            .map(|info| DeviceInfo {
                index: info.index().as_index().unwrap_or(0),
                name: info.human_name().clone(),
                description: info.description().to_string(),
                path: Some(info.misc().clone()),
            })
            .collect();

        Ok(devices)
    }
}

/// Initialise the platform backend exactly once.
///
/// `nokhwa_initialize` is a no-op on Linux but must be called on
/// macOS to satisfy `AVFoundation` permission prompts. Calling it
/// unconditionally keeps the backend portable.
fn ensure_backend_ready() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        nokhwa_initialize(|_granted| {});
    });
}
