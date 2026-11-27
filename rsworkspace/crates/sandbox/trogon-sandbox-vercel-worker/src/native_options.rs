use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{NativeOptionsError, optional_bool, options_for, reject_unknown_keys};

use crate::constants::NATIVE_OPTION_KEYS;

/// Validates `native.vercel` against the allowlist and returns the `persistent` choice. Refusing here, before any
/// sandbox exists, keeps a typo from costing a create call.
pub(crate) fn persistent_option(native: Option<&serde_json::Value>) -> Result<Option<bool>, NativeOptionsError> {
    let options = options_for(ProviderKind::Vercel, native)?;
    reject_unknown_keys(&options, NATIVE_OPTION_KEYS)?;
    optional_bool(&options, "persistent")
}
