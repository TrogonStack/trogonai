//! Env-driven runtime knobs.
//!
//! Pure helpers the gateway boot path reaches for when assembling the
//! policy stack and request-handling defaults. Each helper fails-closed
//! (returns the safer disabled / shorter-deadline default) when the
//! env value is missing or malformed -- callers should branch on the
//! resulting state rather than re-parse the env at the dispatch site.
//!
//! [`gateway_tier3_signing_pubkey`] is the exception, and deliberately
//! so: for a code-signing key the disabled default is *not* the safer
//! one, so it reports a malformed value as its own state instead of
//! folding it into "not configured".

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT;
use a2a_redaction::Ed25519PublicKey;
use tracing::warn;
use trogon_std::env::ReadEnv;

use crate::constants::MESSAGE_SEND_METHOD_DOTS;
pub use crate::constants::{
    ENV_GATEWAY_AUDIT_PUBLISH, ENV_GATEWAY_TIER2_CEL_ENABLED, ENV_GATEWAY_TIER3_SIGNING_PUBKEY,
    ENV_GATEWAY_UNARY_DEADLINE_SECS,
};

/// `true` when Tier-2 CEL evaluation is explicitly enabled via env.
/// Defaults to `false` when the var is unset, missing, or holds a
/// non-truthy value -- the safer default keeps deployments without
/// CEL bundles from paying the engine cost.
pub fn gateway_tier2_cel_enabled<E: ReadEnv>(env: &E) -> bool {
    parse_bool_flag(env, ENV_GATEWAY_TIER2_CEL_ENABLED)
}

/// `true` when ingress audit envelope publishing is explicitly
/// enabled. Defaults to `false` so a deployment doesn't start
/// publishing to a stream operators haven't provisioned.
pub fn gateway_audit_publish_enabled<E: ReadEnv>(env: &E) -> bool {
    parse_bool_flag(env, ENV_GATEWAY_AUDIT_PUBLISH)
}

/// What `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` says about bundle signing.
///
/// The three states are kept distinct because two of them look alike
/// and mean opposite things. "Unset" is an operator who never opted
/// into signing, and unsigned bundles are the expected posture.
/// "Invalid" is an operator who *did* opt in and mistyped the key;
/// collapsing that into "no pubkey configured" would silently execute
/// unverified wasm on a deployment that asked for verification.
#[derive(Debug)]
pub enum Tier3SigningKey {
    /// Var unset or blank: bundle signing was never requested.
    NotConfigured,
    /// A usable verifying key.
    Configured(Ed25519PublicKey),
    /// Var set to something that is not a valid ed25519 pubkey.
    Invalid,
}

impl Tier3SigningKey {
    /// The key to hand the wasm substrate, or `None` when signing was
    /// never configured. [`Self::Invalid`] has no such projection on
    /// purpose: callers have to handle it before they can get here.
    pub fn into_configured(self) -> Option<Ed25519PublicKey> {
        match self {
            Self::Configured(pubkey) => Some(pubkey),
            Self::NotConfigured | Self::Invalid => None,
        }
    }
}

/// Reads the tier-3 wasm bundle signing public key.
///
/// Unlike the other helpers in this module, an unusable value here is
/// not folded into the disabled default: see [`Tier3SigningKey`].
pub fn gateway_tier3_signing_pubkey<E: ReadEnv>(env: &E) -> Tier3SigningKey {
    let Ok(raw) = env.var(ENV_GATEWAY_TIER3_SIGNING_PUBKEY) else {
        return Tier3SigningKey::NotConfigured;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Tier3SigningKey::NotConfigured;
    }
    match Ed25519PublicKey::from_hex(trimmed) {
        Ok(pubkey) => Tier3SigningKey::Configured(pubkey),
        Err(err) => {
            warn!(env_var = ENV_GATEWAY_TIER3_SIGNING_PUBKEY, error = %err, "value is not a valid ed25519 pubkey");
            Tier3SigningKey::Invalid
        }
    }
}

/// Unary deadline for the configured method, in seconds. Only
/// `message.send` carries a deadline today; other methods are
/// either streaming or have their own dispatch-side timeouts.
/// Returns `None` for non-unary methods so the dispatch path can
/// branch without a special-cased value.
pub fn unary_deadline_for_method<E: ReadEnv>(env: &E, method_dots: &str) -> Option<Duration> {
    if method_dots != MESSAGE_SEND_METHOD_DOTS {
        return None;
    }
    let secs: u64 = env
        .var(ENV_GATEWAY_UNARY_DEADLINE_SECS)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_OPERATION_TIMEOUT.as_secs())
        .max(1);
    Some(Duration::from_secs(secs))
}

/// Wall-clock ms since the Unix epoch, clamped to `u64::MAX` if
/// `SystemTime` is somehow set before the epoch. Used as the
/// `published_at` field on audit envelopes -- a monotonic clock
/// would be wrong because we want the wall-clock at publish time.
pub fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Extract the JSON-RPC `params` object from a raw payload. Returns
/// an empty object on any parse failure, missing field, or
/// non-object value (arrays, scalars, `null`) so the caller can
/// rely on `params` always being an object map -- both "no params"
/// and "malformed" shapes downstream behave identically (Tier-3
/// manifest lookups miss, the gate skips the skill).
pub fn json_rpc_params(payload: &[u8]) -> serde_json::Value {
    let extracted = serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.get("params").cloned());
    match extracted {
        Some(value) if value.is_object() => value,
        _ => serde_json::Value::Object(Default::default()),
    }
}

/// Audit-side correlation id derived from the JSON-RPC request id, sharing
/// [`a2a_nats::jsonrpc::correlation_key_from_body`] with the streaming pump so
/// an audit row and the stream it describes join on the same key, and so both
/// agree with the `Trogon-Req-Id` the bridge minted for the same request.
pub fn json_rpc_audit_req_id(payload: &[u8]) -> Option<String> {
    a2a_nats::jsonrpc::correlation_key_from_body(payload)
}

fn parse_bool_flag<E: ReadEnv>(env: &E, key: &str) -> bool {
    match env.var(key) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
