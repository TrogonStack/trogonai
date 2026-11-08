//! The ingress subject carries the only caller principal (ADR#0082). A
//! forwarded caller JWT must name that same principal, and no identity header
//! reaches a policy tier or an agent.
//!
//! The gateway checks tokens and never mints them, so it is configured with
//! the issuer account public keys alone, as a comma-separated list in
//! `A2A_GATEWAY_CALLER_JWT_ISSUER_KEYS` (current first, then the previous key
//! while a rotation overlaps). It reads no signing seed.

use std::sync::Arc;

use a2a_auth_callout::jwt::JwtError;
use a2a_auth_callout::{
    AccountName, AudienceAccount, CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue, IssuerPublicKeys,
    IssuerPublicKeysError, UserJwtClaims, VerifyingKeySource, subject_principal_from_minted_jwt,
};
use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};
use async_nats::header::HeaderName;
use async_nats::{HeaderMap, HeaderValue};
use trogon_identity_types::SubjectPrincipal;
use trogon_std::env::ReadEnv;

pub use crate::constants::{ENV_GATEWAY_CALLER_JWT_ISSUER_KEYS, ENV_GATEWAY_JWT_AUDIENCE};

/// Why the issuer keys the gateway verifies forwarded caller JWTs against
/// could not be loaded. The gateway always verifies (ADR#0082 Decision 3), so
/// each of these stops startup.
#[derive(Debug, thiserror::Error)]
pub enum CallerJwtIssuerKeysError {
    #[error("{ENV_GATEWAY_CALLER_JWT_ISSUER_KEYS} is not set")]
    Missing,
    #[error("{ENV_GATEWAY_CALLER_JWT_ISSUER_KEYS} {0}")]
    Invalid(#[source] IssuerPublicKeysError),
}

/// Reads the account public keys that issue the caller JWTs the gateway
/// accepts. Only public keys are read: the gateway checks tokens and never
/// holds the seed that mints them.
pub fn caller_jwt_issuer_keys_from_env<E: ReadEnv>(env: &E) -> Result<IssuerPublicKeys, CallerJwtIssuerKeysError> {
    let raw = env
        .var(ENV_GATEWAY_CALLER_JWT_ISSUER_KEYS)
        .map_err(|_| CallerJwtIssuerKeysError::Missing)?;
    IssuerPublicKeys::parse(&raw).map_err(CallerJwtIssuerKeysError::Invalid)
}

pub struct ForwardedCallerJwtVerifier {
    issuer_keys: Arc<dyn VerifyingKeySource>,
    audience: AudienceAccount,
}

impl ForwardedCallerJwtVerifier {
    pub fn new(issuer_keys: Arc<dyn VerifyingKeySource>, audience: AudienceAccount) -> Self {
        Self { issuer_keys, audience }
    }

    pub fn check(&self, headers: &HeaderMap, caller: &SubjectPrincipal) -> Result<(), ForwardedCallerJwtError> {
        headers
            .iter()
            .filter(|(name, _)| is_caller_jwt_header(name))
            .flat_map(|(_, values)| values)
            .try_for_each(|value| self.check_value(value, caller))
    }

    fn check_value(&self, value: &HeaderValue, caller: &SubjectPrincipal) -> Result<(), ForwardedCallerJwtError> {
        let token = CallerJwtHeaderValue::parse(value.as_str()).map_err(ForwardedCallerJwtError::Malformed)?;
        UserJwtClaims::verify_minted_user_jwt(token.as_str(), self.issuer_keys.as_ref(), &self.audience)
            .map_err(ForwardedCallerJwtError::Unverified)?;
        let named =
            subject_principal_from_minted_jwt(token.as_str()).map_err(ForwardedCallerJwtError::PrincipalUnreadable)?;
        if &named == caller {
            Ok(())
        } else {
            Err(ForwardedCallerJwtError::PrincipalMismatch {
                named,
                subject: caller.clone(),
            })
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardedCallerJwtError {
    #[error("forwarded caller JWT is not a compact JWT")]
    Malformed(#[source] JwtError),
    #[error("forwarded caller JWT failed verification")]
    Unverified(#[source] JwtError),
    #[error("forwarded caller JWT names no principal")]
    PrincipalUnreadable(#[source] JwtError),
    #[error("forwarded caller JWT names {named}, but the subject carries {subject}")]
    PrincipalMismatch {
        named: SubjectPrincipal,
        subject: SubjectPrincipal,
    },
}

impl ForwardedCallerJwtError {
    pub fn audit_rule(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "gateway.caller_jwt.malformed",
            Self::Unverified(_) => "gateway.caller_jwt.unverified",
            Self::PrincipalUnreadable(_) => "gateway.caller_jwt.principal_unreadable",
            Self::PrincipalMismatch { .. } => "gateway.caller_jwt.principal_mismatch",
        }
    }
}

pub fn without_identity_headers(headers: &HeaderMap) -> HeaderMap {
    let mut kept = HeaderMap::new();
    for (name, values) in headers.iter().filter(|(name, _)| !is_identity_header(name)) {
        for value in values {
            kept.append(name.clone(), value.clone());
        }
    }
    kept
}

fn is_identity_header(name: &HeaderName) -> bool {
    [
        CALLER_JWT_HEADER_NAME,
        GATEWAY_CALLER_ID_HEADER,
        GATEWAY_PRINCIPAL_HEADER,
    ]
    .iter()
    .any(|identity| header_name_is(name, identity))
}

fn is_caller_jwt_header(name: &HeaderName) -> bool {
    header_name_is(name, CALLER_JWT_HEADER_NAME)
}

fn header_name_is(name: &HeaderName, expected: &str) -> bool {
    AsRef::<str>::as_ref(name).eq_ignore_ascii_case(expected)
}

pub fn gateway_jwt_audience<E: ReadEnv>(env: &E, prefix_fallback: &str) -> AudienceAccount {
    env.var(ENV_GATEWAY_JWT_AUDIENCE)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| AccountName::new(s.trim()))
        .unwrap_or_else(|| AccountName::new(prefix_fallback))
}

#[cfg(test)]
mod tests;
