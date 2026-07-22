//! Mint `aa-auth+jwt` auth tokens per "Auth Token Structure" (#auth-tokens),
//! issued by a Person Server (three-party model). Mirrors the sibling
//! Access Server's minting shape (`trogon-aauth-as/src/mint.rs`) since both
//! roles mint the same token type; kept crate-local because the two crates
//! do not depend on each other.

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use trogon_identity_types::aauth::{Act, AuthClaims, Cnf, DWK_PERSON, TYP_AUTH};

use crate::error::MintError;

/// Wire shape actually minted onto the JWT: [`AuthClaims`] flattened plus
/// `dwk`. `AuthClaims` has no `dwk` field yet even though "Auth Token
/// Structure" lists `dwk` as a required claim (`aauth-person.json` for
/// PS-issued tokens) -- flagged for promotion in `lib.rs`. Kept local since it
/// only exists to patch the serialization boundary.
#[derive(Serialize)]
struct MintedAuthClaims<'a> {
    #[serde(flatten)]
    claims: &'a AuthClaims,
    dwk: &'a str,
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

impl Default for AuthTokenTtl {
    fn default() -> Self {
        Self(Self::MAX_SECS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthTokenTtlError {
    #[error("auth token ttl must be positive, got {0}")]
    NotPositive(i64),
    #[error("auth token ttl {0}s exceeds the 1 hour maximum")]
    ExceedsMax(i64),
}

/// Which agent the issued auth token binds to, per "PS Response": ordinarily
/// the requesting agent token's subject, or -- for parent-mediated sub-agent
/// authorization -- the sub-agent token's subject (#sub-agents).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingAgent {
    pub agent: String,
    pub agent_jkt: String,
}

/// Inputs to mint one `aa-auth+jwt`.
pub struct AuthTokenInputs<'a> {
    /// This PS's own issuer URL, per "Auth Token Structure" `iss`.
    pub iss: &'a str,
    /// The resource identifier the token authorizes access to (the resource
    /// token's `iss`), per "PS Response".
    pub aud: &'a str,
    /// Directed user (principal) identifier this PS is acting on behalf of.
    pub sub: &'a str,
    pub jti: &'a str,
    pub binding: &'a BindingAgent,
    pub cnf_jwk: serde_json::Value,
    pub scope: &'a str,
    /// Upstream delegation chain to nest under this token's `act`, per
    /// "Upstream Token Verification" step 4 / "Delegation Chain"
    /// (#delegation-chain). `None` for a direct grant with no chaining.
    pub act: Option<Act>,
    pub principal: Option<&'a str>,
    pub consent_id: Option<&'a str>,
    pub resource: Option<&'a str>,
    pub iat: i64,
    pub ttl: AuthTokenTtl,
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
/// pass `ES256`, `ES384`, or `EdDSA` per "Auth Token Structure" header rules.
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
        scope: inputs.scope.to_string(),
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
        dwk: DWK_PERSON,
    };
    encode(&header, &wire, signing_key).map_err(MintError::Encode)
}

#[cfg(test)]
mod tests;
