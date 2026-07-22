//! Mint `aa-auth+jwt` auth tokens per "Auth Token Structure"
//! (#auth-tokens) and nest the `act` delegation chain per "Upstream Token
//! Verification" (#upstream-token-verification) step 4 / "Delegation Chain"
//! (#delegation-chain).

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use trogon_identity_types::aauth::{Act, AuthClaims, Cnf, DWK_ACCESS, TYP_AUTH};

use crate::error::MintError;
use crate::policy::GrantedScope;
use crate::request::BindingAgent;

/// Wire shape actually minted onto the JWT: [`AuthClaims`] flattened plus
/// `dwk`. `AuthClaims` (in `trogon-identity-types`) has no `dwk` field yet,
/// even though "Auth Token Structure" lists `dwk` as a required claim
/// (`aauth-access.json` for AS-issued tokens, `aauth-person.json` for
/// PS-issued ones) -- see this crate's promotion notes (`lib.rs`). Local to
/// this module rather than a type callers construct directly, since it
/// exists only to patch the gap at the serialization boundary.
#[derive(Serialize)]
struct MintedAuthClaims<'a> {
    #[serde(flatten)]
    claims: &'a AuthClaims,
    dwk: &'a str,
}

/// Inputs to mint one auth token. Kept separate from
/// [`crate::request::AsTokenContext`] because minting only needs the fields
/// that end up on the wire, not the full verified request context policy saw.
pub struct AuthTokenInputs<'a> {
    /// This AS's own issuer URL, per "Auth Token Structure" `iss`.
    pub iss: &'a str,
    /// The resource identifier the token authorizes access to (the resource
    /// token's `iss`), per "Auth Token Delivery" step 3.
    pub aud: &'a str,
    /// Directed user identifier, per "Auth Token Structure" conditional
    /// claims. Required by this crate's mint path; see module docs on the
    /// upstream `AuthClaims.sub` type gap for why `scope`-only tokens are not
    /// yet representable.
    pub sub: &'a str,
    pub jti: &'a str,
    pub binding: &'a BindingAgent,
    pub cnf_jwk: serde_json::Value,
    pub scope: &'a GrantedScope,
    /// Upstream delegation chain to nest under this token's `act`, per
    /// "Upstream Token Verification" step 4. `None` for a direct grant with
    /// no chaining and no sub-agent.
    pub act: Option<Act>,
    pub principal: Option<&'a str>,
    pub consent_id: Option<&'a str>,
    pub resource: Option<&'a str>,
    pub iat: i64,
    /// Token lifetime in seconds. "Auth tokens MUST NOT have a lifetime
    /// exceeding 1 hour" -- enforced by [`AuthTokenTtl`].
    pub ttl: AuthTokenTtl,
}

/// Auth token lifetime in seconds, capped per "Auth Token Structure": "Auth
/// tokens MUST NOT have a lifetime exceeding 1 hour."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthTokenTtl(i64);

impl AuthTokenTtl {
    pub const MAX_SECS: i64 = 3600;

    pub fn new(secs: i64) -> Result<Self, AuthTokenTtlError> {
        if secs <= 0 {
            return Err(AuthTokenTtlError::NotPositive(secs));
        }
        if secs > Self::MAX_SECS {
            return Err(AuthTokenTtlError::ExceedsMax(secs));
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthTokenTtlError {
    #[error("auth token ttl must be positive, got {0}")]
    NotPositive(i64),
    #[error("auth token ttl {0}s exceeds the 1 hour maximum")]
    ExceedsMax(i64),
}

/// Builds `act` for a downstream auth token per "Upstream Token
/// Verification" step 4: `act.agent` is the intermediary resource's agent
/// identifier, and any `act` already on the upstream token nests inside as
/// `act.act`, preserving the full chain (#delegation-chain).
#[must_use]
pub fn nest_act(intermediary_agent: impl Into<String>, upstream_act: Option<Act>) -> Act {
    Act {
        agent: intermediary_agent.into(),
        act: upstream_act.map(Box::new),
    }
}

/// Mints an `aa-auth+jwt`. `alg` MUST NOT be `none`; callers are expected to
/// pass `ES256`, `ES384`, or `EdDSA` per "Auth Token Structure" header rules
/// (mirroring `trogon-aauth-verify`'s accepted algorithm set).
pub fn mint_auth_jwt(
    signing_key: &EncodingKey,
    alg: Algorithm,
    kid: &str,
    inputs: &AuthTokenInputs<'_>,
) -> Result<String, MintError> {
    let iat = inputs.iat;
    let exp = iat.checked_add(inputs.ttl.get()).ok_or(MintError::TtlOverflow {
        iat,
        ttl_secs: inputs.ttl.get(),
    })?;

    let claims = AuthClaims {
        iss: inputs.iss.to_string(),
        sub: inputs.sub.to_string(),
        aud: inputs.aud.to_string(),
        jti: inputs.jti.to_string(),
        iat,
        exp,
        agent: inputs.binding.agent.clone(),
        agent_jkt: inputs.binding.agent_jkt.clone(),
        scope: inputs.scope.as_str().to_string(),
        principal: inputs.principal.map(str::to_string),
        consent_id: inputs.consent_id.map(str::to_string),
        resource: inputs.resource.map(str::to_string),
        act: inputs.act.clone(),
        cnf: Some(Cnf {
            jwk: inputs.cnf_jwk.clone(),
        }),
    };

    let mut header = Header::new(alg);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some(kid.into());

    let wire = MintedAuthClaims {
        claims: &claims,
        dwk: DWK_ACCESS,
    };
    encode(&header, &wire, signing_key).map_err(MintError::Encode)
}

#[cfg(test)]
mod tests;
