use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{NativeOptionsError, optional_bool, optional_u64, options_for, reject_unknown_keys};

use crate::constants::{MAX_INTERVAL_MINUTES, NATIVE_OPTION_KEYS};
use crate::native_params::NativeParams;

fn interval(
    options: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u32>, NativeOptionsError> {
    Ok(optional_u64(options, key, 0, MAX_INTERVAL_MINUTES)?.and_then(|minutes| u32::try_from(minutes).ok()))
}

/// Validates `native.daytona` against the allowlist. Refusing here, before any sandbox exists, keeps a typo from
/// costing a create call.
pub(crate) fn native_params(native: Option<&serde_json::Value>) -> Result<NativeParams, NativeOptionsError> {
    let options = options_for(ProviderKind::Daytona, native)?;
    reject_unknown_keys(&options, NATIVE_OPTION_KEYS)?;
    Ok(NativeParams {
        auto_stop_interval: interval(&options, "auto_stop_interval")?,
        auto_archive_interval: interval(&options, "auto_archive_interval")?,
        auto_delete_interval: interval(&options, "auto_delete_interval")?,
        public: optional_bool(&options, "public")?,
    })
}

#[cfg(test)]
mod tests;
