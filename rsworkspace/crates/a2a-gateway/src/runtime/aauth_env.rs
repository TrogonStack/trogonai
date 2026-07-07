//! Env-driven AAuth ingress layer construction.
//!
//! Builds the [`GatewayAAuthIngress`] the gateway dispatch path checks before
//! Tier-1 authorization runs. Pure config parsing plus file reads -- no live
//! NATS/JetStream binding -- so this module builds and unit-tests in every
//! profile, unlike the `dispatch` module it feeds.

use jsonwebtoken::Algorithm;
use jsonwebtoken::jwk::JwkSet;
use trogon_aauth_verify::{CachedJwksResolver, HttpJwksResolver, SystemTimeSource};
use trogon_std::env::ReadEnv;

use crate::aauth::{
    AAuthConfig, AAuthDenyReason, AAuthIngress, AAuthMode, ChallengeKid, ChallengeKidError, GatewayAAuthIngress,
    GatewayJwksResolver, LeewaySecs, NonNegativeSecs, NonNegativeSecsError, PersonServerAudience,
    PersonServerAudienceError, ResourceIssuer, ResourceIssuerError, StaticJwks,
};

pub const ENV_AAUTH_MODE: &str = "A2A_GATEWAY_AAUTH_MODE";
pub const ENV_AAUTH_JWKS_PATH: &str = "A2A_GATEWAY_AAUTH_JWKS_PATH";
pub const ENV_AAUTH_JWKS_DISCOVERY: &str = "A2A_GATEWAY_AAUTH_JWKS_DISCOVERY";
pub const ENV_AAUTH_JWKS_TTL_SECS: &str = "A2A_GATEWAY_AAUTH_JWKS_TTL_SECS";
pub const ENV_AAUTH_RESOURCE_ISS: &str = "A2A_GATEWAY_AAUTH_RESOURCE_ISS";
pub const ENV_AAUTH_PERSON_SERVER_AUD: &str = "A2A_GATEWAY_AAUTH_PERSON_SERVER_AUD";
pub const ENV_AAUTH_CHALLENGE_KID: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_KID";
pub const ENV_AAUTH_CHALLENGE_KEY_PATH: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_KEY_PATH";
pub const ENV_AAUTH_LEEWAY_SECS: &str = "A2A_GATEWAY_AAUTH_LEEWAY_SECS";
pub const ENV_AAUTH_CHALLENGE_TTL_SECS: &str = "A2A_GATEWAY_AAUTH_CHALLENGE_TTL_SECS";
pub const ENV_AAUTH_MAX_SKEW_SECS: &str = "A2A_GATEWAY_AAUTH_MAX_SKEW_SECS";

/// Audit `caller_source` recorded once a verified `aa-auth+jwt` principal
/// supersedes the JWT-header caller identity for the remainder of dispatch.
pub const AAUTH_CALLER_SOURCE: &str = "aauth";

const DEFAULT_LEEWAY_SECS: u64 = 60;
const DEFAULT_CHALLENGE_TTL_SECS: i64 = 300;
const DEFAULT_MAX_SKEW_SECS: i64 = 60;

/// Every variant names the exact env var an operator needs to fix -- shadow
/// and enforce mode must never silently fall back to a Noop/Off layer just
/// because a required var was missing or malformed.
#[derive(Debug, thiserror::Error)]
pub enum AAuthEnvError {
    #[error("{ENV_AAUTH_MODE} must be one of off|shadow|enforce, got {0:?}")]
    InvalidMode(String),
    #[error("{ENV_AAUTH_MODE}=shadow|enforce requires {0} to be set")]
    MissingRequired(&'static str),
    #[error("failed to read {var} at {path}: {source}")]
    ReadFile {
        var: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {var} JSON at {path}: {source}")]
    ParseJwks {
        var: &'static str,
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to build EC encoding key from {ENV_AAUTH_CHALLENGE_KEY_PATH}: {0}")]
    InvalidChallengeKey(#[source] jsonwebtoken::errors::Error),
    #[error("{0}: {1}")]
    InvalidResourceIssuer(&'static str, #[source] ResourceIssuerError),
    #[error("{0}: {1}")]
    InvalidPersonServerAudience(&'static str, #[source] PersonServerAudienceError),
    #[error("{0}: {1}")]
    InvalidChallengeKid(&'static str, #[source] ChallengeKidError),
    #[error("{var} must be a non-negative integer, got {raw:?}")]
    InvalidNonNegativeSecs {
        var: &'static str,
        raw: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("{var} must be non-negative, got {raw:?}")]
    NegativeSecs {
        var: &'static str,
        raw: String,
        #[source]
        source: NonNegativeSecsError,
    },
    #[error(
        "exactly one of {ENV_AAUTH_JWKS_PATH} or {ENV_AAUTH_JWKS_DISCOVERY}=true must be configured when \
         {ENV_AAUTH_MODE} is shadow|enforce, got neither"
    )]
    MissingJwksSource,
    #[error(
        "exactly one of {ENV_AAUTH_JWKS_PATH} or {ENV_AAUTH_JWKS_DISCOVERY}=true must be configured when \
         {ENV_AAUTH_MODE} is shadow|enforce, got both"
    )]
    ConflictingJwksSource,
    #[error("{ENV_AAUTH_JWKS_DISCOVERY} must be one of true|false, got {0:?}")]
    InvalidJwksDiscovery(String),
}

/// Resolve the [`GatewayAAuthIngress`] layer from environment.
///
/// Returns `Ok(None)` when AAuth is off (the default) so the dispatch path
/// can skip the verifier entirely. Shadow / enforce mode require every
/// listed var to be present and valid -- misconfiguration is a typed error
/// naming the offending var rather than a silent fallback to Off, matching
/// the "fail loudly when enabled but misconfigured" pattern the Tier-1
/// SpiceDB layer uses.
pub fn gateway_aauth_from_env<E: ReadEnv>(env: &E) -> Result<Option<GatewayAAuthIngress>, AAuthEnvError> {
    let mode = aauth_mode_from_env(env)?;
    if mode == AAuthMode::Off {
        return Ok(None);
    }

    let resource_iss_raw = required_var(env, ENV_AAUTH_RESOURCE_ISS)?;
    let person_server_aud_raw = required_var(env, ENV_AAUTH_PERSON_SERVER_AUD)?;
    let challenge_kid_raw = required_var(env, ENV_AAUTH_CHALLENGE_KID)?;
    let challenge_key_path = required_var(env, ENV_AAUTH_CHALLENGE_KEY_PATH)?;

    let jwks = jwks_resolver_from_env(env)?;
    let challenge_key = load_challenge_key(&challenge_key_path)?;

    let resource_iss = ResourceIssuer::new(resource_iss_raw)
        .map_err(|e| AAuthEnvError::InvalidResourceIssuer(ENV_AAUTH_RESOURCE_ISS, e))?;
    let person_server_aud = PersonServerAudience::new(person_server_aud_raw)
        .map_err(|e| AAuthEnvError::InvalidPersonServerAudience(ENV_AAUTH_PERSON_SERVER_AUD, e))?;
    let challenge_kid = ChallengeKid::new(challenge_kid_raw)
        .map_err(|e| AAuthEnvError::InvalidChallengeKid(ENV_AAUTH_CHALLENGE_KID, e))?;

    let leeway_secs = LeewaySecs::new(optional_u64(env, ENV_AAUTH_LEEWAY_SECS, DEFAULT_LEEWAY_SECS)?);
    let challenge_ttl_secs = non_negative_secs(
        ENV_AAUTH_CHALLENGE_TTL_SECS,
        optional_i64(env, ENV_AAUTH_CHALLENGE_TTL_SECS, DEFAULT_CHALLENGE_TTL_SECS)?,
    )?;
    let max_skew_secs = non_negative_secs(
        ENV_AAUTH_MAX_SKEW_SECS,
        optional_i64(env, ENV_AAUTH_MAX_SKEW_SECS, DEFAULT_MAX_SKEW_SECS)?,
    )?;

    let cfg = AAuthConfig {
        mode,
        jwks,
        resource_iss,
        person_server_aud,
        leeway_secs,
        challenge_alg: Algorithm::ES256,
        challenge_key,
        challenge_kid,
        challenge_ttl_secs,
        max_skew_secs,
    };
    Ok(Some(std::sync::Arc::new(AAuthIngress::new_in_memory(cfg))))
}

/// Maps a denial reason to the audit `rules_fired` entry dispatch.rs
/// records for an AAuth denial. Kept outside the `not(coverage)`-gated
/// dispatch module so this branch stays covered under coverage builds.
pub fn aauth_deny_rule_fired(reason: &AAuthDenyReason) -> &'static str {
    match reason {
        AAuthDenyReason::Pop(_) => "gateway.aauth.denied.pop",
        AAuthDenyReason::Auth(_) => "gateway.aauth.denied.auth",
        AAuthDenyReason::AuthAgentMismatch { .. } => "gateway.aauth.denied.auth_agent_mismatch",
        AAuthDenyReason::ScopeNotCovered { .. } => "gateway.aauth.denied.scope",
        AAuthDenyReason::MissionMismatch(_) => "gateway.aauth.denied.mission",
        AAuthDenyReason::MissionHeaderMissing { .. } => "gateway.aauth.denied.mission_header_missing",
    }
}

/// Caller identity fields dispatch threads through Tier-1 and into audit
/// envelopes: the SpiceDB-shaped `audit_caller_id` (e.g. `"user/alice"` or
/// the anonymous sentinel), the audit-facing attribution source, and the
/// bare `caller_slug` used to build the Tier-1 principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCallerIdentity {
    pub audit_caller_id: String,
    pub audit_caller_source: Option<String>,
    pub caller_slug: Option<String>,
}

/// Decide whether a verified AAuth principal supersedes the JWT-header
/// caller identity resolved earlier in dispatch.
///
/// An `aa-auth+jwt` is optional even when the agent's `aa-agent+jwt` + PoP
/// verified -- an agent can authenticate on its own behalf with no human
/// principal behind it. Per the AAuth draft, when a principal IS presented
/// and verified, it identifies who the agent is acting for and must take
/// over caller attribution for authorization (Tier-1) and audit; when it is
/// absent, the existing JWT-header-derived identity is left untouched
/// because there is nothing more authoritative to switch to.
///
/// A pure function (no I/O, no dispatch types) so the supersede decision is
/// unit-testable without constructing the `not(coverage)`-gated dispatch
/// plumbing.
#[must_use]
pub fn gateway_caller_identity_after_aauth(
    existing: GatewayCallerIdentity,
    principal: Option<&str>,
) -> GatewayCallerIdentity {
    let Some(principal) = principal else {
        return existing;
    };
    GatewayCallerIdentity {
        audit_caller_id: format!("user/{principal}"),
        audit_caller_source: Some(AAUTH_CALLER_SOURCE.to_owned()),
        caller_slug: Some(principal.to_owned()),
    }
}

fn aauth_mode_from_env<E: ReadEnv>(env: &E) -> Result<AAuthMode, AAuthEnvError> {
    let raw = match env.var(ENV_AAUTH_MODE) {
        Ok(raw) => raw,
        Err(_) => return Ok(AAuthMode::Off),
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "off" => Ok(AAuthMode::Off),
        "shadow" => Ok(AAuthMode::Shadow),
        "enforce" => Ok(AAuthMode::Enforce),
        _ => Err(AAuthEnvError::InvalidMode(raw)),
    }
}

/// Resolve the JWKS source configured for shadow/enforce mode.
///
/// Exactly one of `A2A_GATEWAY_AAUTH_JWKS_PATH` (a static JWKS file) or
/// `A2A_GATEWAY_AAUTH_JWKS_DISCOVERY=true` (HTTP well-known discovery, TTL
/// cached) must be set. Neither or both is a misconfiguration -- fail loudly
/// rather than silently pick one, since an operator flipping on discovery
/// without removing a stale static path (or vice versa) would otherwise
/// resolve to whichever branch happens to be checked first.
fn jwks_resolver_from_env<E: ReadEnv>(env: &E) -> Result<GatewayJwksResolver, AAuthEnvError> {
    let jwks_path = optional_non_empty_var(env, ENV_AAUTH_JWKS_PATH);
    let discovery_enabled = jwks_discovery_enabled(env)?;

    match (jwks_path, discovery_enabled) {
        (Some(path), false) => Ok(GatewayJwksResolver::Static(load_static_jwks(&path)?)),
        (None, true) => Ok(GatewayJwksResolver::WellKnown(std::sync::Arc::new(
            well_known_resolver_from_env(env)?,
        ))),
        (None, false) => Err(AAuthEnvError::MissingJwksSource),
        (Some(_), true) => Err(AAuthEnvError::ConflictingJwksSource),
    }
}

fn well_known_resolver_from_env<E: ReadEnv>(
    env: &E,
) -> Result<CachedJwksResolver<HttpJwksResolver, SystemTimeSource>, AAuthEnvError> {
    // A negative TTL is not "expire immediately", it is a misconfiguration
    // that would turn every verification into a discovery fetch -- reject it
    // at startup like the other second-count vars.
    let ttl_secs = non_negative_secs(
        ENV_AAUTH_JWKS_TTL_SECS,
        optional_i64(env, ENV_AAUTH_JWKS_TTL_SECS, trogon_aauth_verify::DEFAULT_TTL_SECS)?,
    )?
    .get();
    Ok(CachedJwksResolver::new(HttpJwksResolver::new(), SystemTimeSource).with_ttl_secs(ttl_secs))
}

fn jwks_discovery_enabled<E: ReadEnv>(env: &E) -> Result<bool, AAuthEnvError> {
    match env.var(ENV_AAUTH_JWKS_DISCOVERY) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(AAuthEnvError::InvalidJwksDiscovery(raw)),
        },
        Err(_) => Ok(false),
    }
}

fn optional_non_empty_var<E: ReadEnv>(env: &E, key: &str) -> Option<String> {
    env.var(key).ok().filter(|v| !v.trim().is_empty())
}

fn required_var<E: ReadEnv>(env: &E, key: &'static str) -> Result<String, AAuthEnvError> {
    // Return the trimmed value, not the original: env vars mounted from
    // secret files commonly carry a trailing newline, which would otherwise
    // leak into file paths and issuer/kid values.
    match env.var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(AAuthEnvError::MissingRequired(key)),
    }
}

fn load_static_jwks(path: &str) -> Result<StaticJwks, AAuthEnvError> {
    let raw = std::fs::read_to_string(path).map_err(|source| AAuthEnvError::ReadFile {
        var: ENV_AAUTH_JWKS_PATH,
        path: path.to_owned(),
        source,
    })?;
    let parsed: std::collections::HashMap<String, JwkSet> =
        serde_json::from_str(&raw).map_err(|source| AAuthEnvError::ParseJwks {
            var: ENV_AAUTH_JWKS_PATH,
            path: path.to_owned(),
            source,
        })?;
    let mut jwks = StaticJwks::new();
    for (iss, set) in parsed {
        jwks.insert(iss, set);
    }
    Ok(jwks)
}

fn load_challenge_key(path: &str) -> Result<jsonwebtoken::EncodingKey, AAuthEnvError> {
    let pem = std::fs::read(path).map_err(|source| AAuthEnvError::ReadFile {
        var: ENV_AAUTH_CHALLENGE_KEY_PATH,
        path: path.to_owned(),
        source,
    })?;
    jsonwebtoken::EncodingKey::from_ec_pem(&pem).map_err(AAuthEnvError::InvalidChallengeKey)
}

fn optional_u64<E: ReadEnv>(env: &E, key: &'static str, default: u64) -> Result<u64, AAuthEnvError> {
    match env.var(key) {
        Ok(raw) => raw
            .trim()
            .parse::<u64>()
            .map_err(|source| AAuthEnvError::InvalidNonNegativeSecs { var: key, raw, source }),
        Err(_) => Ok(default),
    }
}

fn optional_i64<E: ReadEnv>(env: &E, key: &'static str, default: i64) -> Result<i64, AAuthEnvError> {
    match env.var(key) {
        Ok(raw) => raw
            .trim()
            .parse::<i64>()
            .map_err(|source| AAuthEnvError::InvalidNonNegativeSecs { var: key, raw, source }),
        Err(_) => Ok(default),
    }
}

fn non_negative_secs(var: &'static str, secs: i64) -> Result<NonNegativeSecs, AAuthEnvError> {
    NonNegativeSecs::new(secs).map_err(|source| AAuthEnvError::NegativeSecs {
        var,
        raw: secs.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests;
