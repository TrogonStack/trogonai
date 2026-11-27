//! Vendor passthrough options carried by `LeaseRequest::native`.
//!
//! The wire shape is an object keyed by provider token. The broker forwards
//! only the entry of the provider it picked, and that provider's worker checks
//! it against its own allowlist with the helpers here, so an option the worker
//! does not document is refused rather than ignored.

use serde_json::{Map, Value};
use trogon_sandbox_contract::{ProviderDispatchReply, ProviderKind};

use crate::native_options_error::NativeOptionsError;
use crate::provider_error::ProviderError;
use crate::subjects::provider_token;

/// The options meant for `provider`: the object under its token. `None` yields
/// an empty map. Options keyed to another provider are an error here because
/// the broker strips them before dispatch, so seeing one means a caller went
/// around the broker.
pub fn options_for(provider: ProviderKind, native: Option<&Value>) -> Result<Map<String, Value>, NativeOptionsError> {
    let Some(native) = native else {
        return Ok(Map::new());
    };
    let Some(object) = native.as_object() else {
        return Err(NativeOptionsError::new("native must be an object keyed by provider"));
    };
    let token = provider_token(provider);
    if let Some(other) = object.keys().find(|key| key.as_str() != token) {
        return Err(NativeOptionsError::new(format!(
            "native options for '{other}' reached the '{token}' worker"
        )));
    }
    match object.get(token) {
        None => Ok(Map::new()),
        Some(Value::Object(options)) => Ok(options.clone()),
        Some(_) => Err(NativeOptionsError::new(format!("native.{token} must be an object"))),
    }
}

/// Fails on the first key not in `allowed`, naming the key and the accepted set.
pub fn reject_unknown_keys(options: &Map<String, Value>, allowed: &[&str]) -> Result<(), NativeOptionsError> {
    match options.keys().find(|key| !allowed.contains(&key.as_str())) {
        None => Ok(()),
        Some(key) => Err(NativeOptionsError::new(format!(
            "unknown native option '{key}'; accepted: {}",
            allowed.join(", ")
        ))),
    }
}

/// Reads an optional unsigned integer in `min..=max`.
pub fn optional_u64(
    options: &Map<String, Value>,
    key: &str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, NativeOptionsError> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    match value.as_u64() {
        Some(n) if (min..=max).contains(&n) => Ok(Some(n)),
        _ => Err(NativeOptionsError::new(format!(
            "native option '{key}' must be an integer between {min} and {max}"
        ))),
    }
}

/// Reads an optional string that must be one of `allowed`.
pub fn optional_choice(
    options: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, NativeOptionsError> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    match value.as_str() {
        Some(text) if allowed.contains(&text) => Ok(Some(text.to_string())),
        _ => Err(NativeOptionsError::new(format!(
            "native option '{key}' must be one of: {}",
            allowed.join(", ")
        ))),
    }
}

/// Reads an optional boolean.
pub fn optional_bool(options: &Map<String, Value>, key: &str) -> Result<Option<bool>, NativeOptionsError> {
    let Some(value) = options.get(key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| NativeOptionsError::new(format!("native option '{key}' must be a boolean")))
}

/// The reply a worker sends back when [`Provider::lease`](crate::Provider::lease) fails:
/// `native_error` for a refused `native` payload, `provider_error` for anything else.
pub fn dispatch_failure_reply(error: &ProviderError) -> ProviderDispatchReply {
    match error {
        ProviderError::NativeOptions(native) => ProviderDispatchReply::NativeRejected {
            native_error: native.reason().to_owned(),
        },
        other => ProviderDispatchReply::LaunchFailed {
            provider_error: other.to_string(),
        },
    }
}
