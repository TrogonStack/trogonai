use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{DispatchAuth, FileSha256, FileSha256Error};

use crate::dispatch_auth_constants::SIGNING_DOMAIN;

/// The bytes the broker signs for an envelope.
///
/// Every field is length-prefixed so no two envelopes share a signing input. The nonce and epoch are signed only
/// when present.
pub(crate) fn signing_input(auth: &DispatchAuth) -> Vec<u8> {
    let mut out = String::from(SIGNING_DOMAIN);
    out.push('\n');
    let mut field = |name: &str, value: &str| {
        let _ = writeln!(out, "{name}={}:{value}", value.len());
    };
    field("version", &auth.version.to_string());
    field("key_id", &auth.key_id);
    field("operation", &auth.operation);
    field("tenant", auth.tenant.as_str());
    field("lease_id", auth.lease_id.as_str());
    field(
        "worker_id",
        auth.worker_id.as_ref().map_or("", |worker| worker.as_str()),
    );
    field(
        "attempt_id",
        auth.attempt_id.as_ref().map_or("", |attempt| attempt.as_str()),
    );
    if let Some(nonce) = &auth.nonce {
        field("nonce", nonce);
    }
    if let Some(epoch) = auth.epoch {
        field("epoch", &epoch.get().to_string());
    }
    field("expires_at", &auth.expires_at.to_string());
    field("body_sha256", auth.body_sha256.as_str());
    out.into_bytes()
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The checksum of `bytes` as a parsed value.
pub(crate) fn sha256_digest(bytes: &[u8]) -> Result<FileSha256, FileSha256Error> {
    FileSha256::new(sha256_hex(bytes))
}

/// Hex SHA-256 of a dispatch object serialized without its envelope.
pub(crate) fn body_digest(object: &serde_json::Map<String, Value>) -> String {
    // A `Value` map always serializes.
    let bytes = serde_json::to_vec(object).unwrap_or_default();
    sha256_hex(&bytes)
}

/// Whole seconds since the Unix epoch; zero for a time before it.
pub(crate) fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_secs())
}

/// The string at a JSON pointer, if the value there is a string.
pub(crate) fn string_at(value: &Value, pointer: &str) -> Option<String> {
    pointer_str(value, pointer).map(str::to_owned)
}

/// The borrowed string at a JSON pointer, if the value there is a string.
pub(crate) fn pointer_str<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}
