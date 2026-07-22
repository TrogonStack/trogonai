//! Reads the `parent_agent` claim from an already-verified `aa-agent+jwt`,
//! per "Agent Token Structure" (#agent-tokens) optional payload claim and
//! "Sub-Agents" (#sub-agents).
//!
//! [`trogon_identity_types::aauth::AgentClaims`] does not yet model
//! `parent_agent` (flagged in `lib.rs` for promotion), so this module decodes
//! it directly off the verified JWT's payload segment rather than widening
//! that type from this crate. [`trogon_aauth_verify::TokenVerifier`] has
//! already verified the token's signature and freshness by the time this
//! runs, so re-parsing the (trusted) payload for one extra optional field
//! does not weaken verification.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ParentAgentError {
    #[error("malformed JWT: expected three dot-separated segments")]
    BadShape,
    #[error("payload segment is not valid base64url: {0}")]
    Base64(#[source] base64::DecodeError),
    #[error("payload segment is not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
}

#[derive(Deserialize)]
struct ParentAgentOnly {
    #[serde(default)]
    parent_agent: Option<String>,
}

/// Extract `parent_agent` from an `aa-agent+jwt`'s payload, if present.
/// Callers MUST have already verified the token's signature; this function
/// performs no cryptographic verification of its own.
pub fn parent_agent_of(verified_jwt: &str) -> Result<Option<String>, ParentAgentError> {
    let mut parts = verified_jwt.splitn(3, '.');
    let _header = parts.next().ok_or(ParentAgentError::BadShape)?;
    let payload_b64 = parts.next().ok_or(ParentAgentError::BadShape)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(ParentAgentError::Base64)?;
    let parsed: ParentAgentOnly = serde_json::from_slice(&payload).map_err(ParentAgentError::Json)?;
    Ok(parsed.parent_agent)
}

#[cfg(test)]
mod tests;
