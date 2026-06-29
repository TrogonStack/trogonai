//! Env-driven runtime knobs.
//!
//! Pure helpers the gateway boot path reaches for when assembling the
//! policy stack and request-handling defaults. Each helper fails-closed
//! (returns the safer disabled / shorter-deadline default) when the
//! env value is missing or malformed -- callers should branch on the
//! resulting state rather than re-parse the env at the dispatch site.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a2a_nats::constants::DEFAULT_OPERATION_TIMEOUT;
use a2a_redaction::Ed25519PublicKey;
use tracing::warn;
use trogon_std::env::ReadEnv;

pub const ENV_GATEWAY_TIER2_CEL_ENABLED: &str = "A2A_GATEWAY_TIER2_CEL_ENABLED";
pub const ENV_GATEWAY_TIER3_SIGNING_PUBKEY: &str = "A2A_GATEWAY_TIER3_SIGNING_PUBKEY";
pub const ENV_GATEWAY_AUDIT_PUBLISH: &str = "A2A_GATEWAY_AUDIT_PUBLISH";
pub const ENV_GATEWAY_UNARY_DEADLINE_SECS: &str = "A2A_GATEWAY_UNARY_DEADLINE_SECS";

const MESSAGE_SEND_METHOD_DOTS: &str = "message.send";

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

/// Tier-3 wasm bundle signing public key, if configured. Returns
/// `None` for unset / empty / unparseable values -- the gateway
/// then refuses to verify signatures rather than half-trusting an
/// invalid pubkey.
pub fn gateway_tier3_signing_pubkey<E: ReadEnv>(env: &E) -> Option<Ed25519PublicKey> {
    let Ok(raw) = env.var(ENV_GATEWAY_TIER3_SIGNING_PUBKEY) else {
        return None;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match Ed25519PublicKey::from_hex(trimmed) {
        Ok(pubkey) => Some(pubkey),
        Err(err) => {
            warn!(
                error = %err,
                "{ENV_GATEWAY_TIER3_SIGNING_PUBKEY} invalid; tier-3 bundle signing disabled",
            );
            None
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
/// an empty object on any parse failure so the caller doesn't have
/// to special-case "no params" vs "malformed" -- both shapes
/// downstream behave identically (Tier-3 manifest lookups miss, the
/// gate skips the skill).
pub fn json_rpc_params(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.get("params").cloned())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()))
}

/// Audit-side correlation id derived from the JSON-RPC request id
/// header. Returns `None` when the payload doesn't carry an id (a
/// notification or malformed envelope) so the audit consumer can
/// route those to the no-correlation bucket without trying to join
/// on a synthesized key.
pub fn json_rpc_audit_req_id(payload: &[u8]) -> Option<String> {
    a2a_nats::jsonrpc::extract_request_id_from_body(payload).map(|id| id.to_string())
}

fn parse_bool_flag<E: ReadEnv>(env: &E, key: &str) -> bool {
    match env.var(key) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
