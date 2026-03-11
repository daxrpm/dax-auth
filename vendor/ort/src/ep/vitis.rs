use alloc::string::ToString;

use super::{ExecutionProvider, ExecutionProviderOptions, RegisterError};
use crate::{error::Result, session::builder::SessionBuilder};

#[derive(Debug, Default, Clone)]
pub struct Vitis {
    options: ExecutionProviderOptions,
}

super::impl_ep!(arbitrary; Vitis);

impl Vitis {
    pub fn with_config_file(mut self, config_file: impl ToString) -> Self {
        self.options.set("config_file", config_file.to_string());
        self
    }

    pub fn with_cache_dir(mut self, cache_dir: impl ToString) -> Self {
        self.options.set("cache_dir", cache_dir.to_string());
        self
    }

    pub fn with_cache_key(mut self, cache_key: impl ToString) -> Self {
        self.options.set("cache_key", cache_key.to_string());
        self
    }
}

impl ExecutionProvider for Vitis {
    fn name(&self) -> &'static str {
        "VitisAIExecutionProvider"
    }

    fn supported_by_platform(&self) -> bool {
        cfg!(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64")
        ))
    }

    #[allow(unused, unreachable_code)]
    fn register(&self, session_builder: &mut SessionBuilder) -> Result<(), RegisterError> {
        // NOTE: SessionOptionsAppendExecutionProvider_VitisAI requires OrtApi v18+
        // (feature "api-18" in ort-sys). Using "load-dynamic" alone does not expose this
        // field — this is a bug in ort rc.12. Gate on "vitis" feature only (not load-dynamic).
        // See: https://github.com/pykeio/ort/issues/... (tracked upstream)
        #[cfg(feature = "vitis")]
        {
            use crate::{ortsys, AsPointer};

            let ffi_options = self.options.to_ffi();
            ortsys![
                unsafe SessionOptionsAppendExecutionProvider_VitisAI(
                    session_builder.ptr_mut(),
                    ffi_options.key_ptrs(),
                    ffi_options.value_ptrs(),
                    ffi_options.len()
                )?
            ];
            return Ok(());
        }

        Err(RegisterError::MissingFeature)
    }
}
