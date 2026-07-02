//! Deterministic canonicalization of event bytes prior to hashing.

use std::sync::Arc;

use serde_json::Value;

/// The exact byte sequence that is hashed into the audit chain for one event.
///
/// Hashing raw, caller-supplied bytes directly would make the chain brittle:
/// two producers that serialize the same logical event with different key
/// order, whitespace, or numeric formatting would compute different hashes
/// for what is semantically the same event, and `verify_chain` would report
/// tampering that never happened. `CanonicalEventBytes` fixes one
/// canonicalization rule so every producer and verifier hashes the same
/// bytes for the same event.
///
/// Canonicalization rule (mirrors AUDIT-COMPLIANCE-1.0 Section 4.4's entry
/// hash algorithm): the input must already be a JSON object or array. It is
/// parsed and re-serialized with object keys sorted alphabetically (recursing
/// into nested objects and arrays) and no insignificant whitespace. The
/// canonical bytes are the UTF-8 encoding of that re-serialization.
///
/// This is deliberately not "hash whatever bytes you hand me": callers that
/// need chain interoperability must serialize events as JSON before calling
/// [`CanonicalEventBytes::from_json_slice`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalEventBytes(Arc<[u8]>);

impl CanonicalEventBytes {
    /// Parses `bytes` as JSON and produces the canonical form.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, CanonicalizeError> {
        let value: Value = serde_json::from_slice(bytes).map_err(CanonicalizeError::InvalidJson)?;
        Self::from_json_value(&value)
    }

    /// Canonicalizes an already-parsed JSON value.
    pub fn from_json_value(value: &Value) -> Result<Self, CanonicalizeError> {
        let sorted = sort_keys(value);
        let encoded = serde_json::to_vec(&sorted).map_err(CanonicalizeError::Reserialize)?;
        Ok(Self(Arc::from(encoded)))
    }

    /// Returns the canonical bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Recursively rebuilds `value` so every object serializes with sorted keys.
///
/// `serde_json::Value::Object` is backed by a map that already iterates in
/// insertion order (or sorted order, depending on the `preserve_order`
/// feature); relying on that implicitly would make canonicalization depend on
/// how the caller built the `Value`. Rebuilding into a `BTreeMap` makes the
/// sort explicit and independent of `serde_json`'s default map type.
fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, Value> =
                map.iter().map(|(key, value)| (key.clone(), sort_keys(value))).collect();
            let mut object = serde_json::Map::with_capacity(sorted.len());
            for (key, value) in sorted {
                object.insert(key, value);
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Error returned when canonicalizing event bytes fails.
#[derive(Debug, thiserror::Error)]
pub enum CanonicalizeError {
    /// The input bytes were not valid JSON.
    #[error("event bytes are not valid JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),
    /// The canonicalized value failed to re-serialize.
    ///
    /// This should not happen for values produced by `serde_json::from_slice`,
    /// but the fallible path is kept explicit rather than unwrapped.
    #[error("failed to re-serialize canonical event value: {0}")]
    Reserialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests;
