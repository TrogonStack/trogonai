//! AAuth proof of possession at the bridge's HTTP edge (ADR#0082 Decision 4).
//!
//! Every request is checked before the bridge asks the auth callout to mint
//! for it, so the callout only ever mints for the caller whose key signed the
//! request.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use trogon_aauth_verify::{HttpPopVerifier, HttpRequest, JwksResolver, ReplayStore, TimeSource};

use crate::edge_identity::VerifiedEdgeCaller;
use crate::error::EdgeAuthError;
use crate::identity::CallerHttpsAuth;
use crate::request::InboundRequest;

mod config;

pub use crate::constants::{
    ENV_AAUTH_AUTHORITY, ENV_AAUTH_CHALLENGE_KEY_PATH, ENV_AAUTH_CHALLENGE_KID, ENV_AAUTH_DEV_UNENFORCED,
    ENV_AAUTH_JWKS_ALLOWED_ISSUERS, ENV_AAUTH_JWKS_DISCOVERY, ENV_AAUTH_JWKS_PATH, ENV_AAUTH_JWKS_TTL_SECS,
    ENV_AAUTH_LEEWAY_SECS, ENV_AAUTH_MAX_SKEW_SECS, ENV_AAUTH_MODE, ENV_AAUTH_NONCE, ENV_AAUTH_NONCE_TTL_SECS,
    ENV_AAUTH_RESOURCE_ISS, ENV_AAUTH_SIGNATURE_VALIDITY_SECS,
};
pub use config::{EdgeAuthConfig, EdgeAuthConfigError, ProofSettings};

/// How strictly the edge holds a request to its AAuth proof of possession.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeAuthMode {
    /// Refuses every request whose proof does not verify for the caller its
    /// bearer credential names.
    Enforce,
    /// Verifies and logs, refusing only a request that carries no proof or
    /// names no caller.
    Shadow,
    /// Does not look at the proof at all.
    Off,
}

impl EdgeAuthMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Shadow => "shadow",
            Self::Off => "off",
        }
    }
}

impl fmt::Display for EdgeAuthMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// The modes that verify a proof: what the edge does when one fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofPolicy {
    Enforce,
    Shadow,
}

impl ProofPolicy {
    #[must_use]
    pub fn mode(self) -> EdgeAuthMode {
        match self {
            Self::Enforce => EdgeAuthMode::Enforce,
            Self::Shadow => EdgeAuthMode::Shadow,
        }
    }
}

impl fmt::Display for ProofPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.mode().fmt(f)
    }
}

/// The host, and optional port, clients reach the edge at. Every signature
/// covers it as `@authority`, so a request signed for another host does not
/// verify here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeAuthority(String);

impl EdgeAuthority {
    /// # Errors
    ///
    /// [`EdgeAuthorityError`] when `raw` is empty or carries anything beyond
    /// a host and port, such as a scheme, path or user info.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EdgeAuthorityError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EdgeAuthorityError::Empty);
        }
        if trimmed
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '?' | '#' | '@'))
        {
            return Err(EdgeAuthorityError::NotAnAuthority(trimmed.to_owned()));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EdgeAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdgeAuthorityError {
    #[error("authority is empty")]
    Empty,
    #[error("{0:?} is not a host with an optional port")]
    NotAnAuthority(String),
}

/// The edge's own identifier: the audience an auth token has to name and the
/// issuer of the nonces the edge hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIdentifier(String);

impl ResourceIdentifier {
    /// # Errors
    ///
    /// [`EmptyResourceIdentifierError`] when `raw` is empty.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EmptyResourceIdentifierError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EmptyResourceIdentifierError);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("resource identifier is empty")]
pub struct EmptyResourceIdentifierError;

/// Why the edge let a request through to the mint.
#[derive(Debug)]
pub enum EdgeAdmission {
    /// The proof verified for the caller the bearer credential names.
    Verified(VerifiedEdgeCaller),
    /// The proof failed, and shadow mode let the request through anyway.
    Shadowed(EdgeAuthError),
    /// The edge does not check proofs.
    Unchecked,
}

#[async_trait]
pub trait EdgeGuard: Send + Sync {
    /// # Errors
    ///
    /// [`EdgeAuthError`] when the request may not reach the mint.
    async fn admit(&self, request: &InboundRequest, caller: &CallerHttpsAuth) -> Result<EdgeAdmission, EdgeAuthError>;

    fn mode(&self) -> EdgeAuthMode;
}

/// The guard of [`EdgeAuthMode::Off`], which admits every request unchecked.
#[derive(Debug, Clone, Copy, Default)]
pub struct BearerOnlyGuard;

#[async_trait]
impl EdgeGuard for BearerOnlyGuard {
    async fn admit(
        &self,
        _request: &InboundRequest,
        _caller: &CallerHttpsAuth,
    ) -> Result<EdgeAdmission, EdgeAuthError> {
        Ok(EdgeAdmission::Unchecked)
    }

    fn mode(&self) -> EdgeAuthMode {
        EdgeAuthMode::Off
    }
}

pub type EdgePopVerifier = HttpPopVerifier<Arc<dyn JwksResolver>, Arc<dyn TimeSource>, Arc<dyn ReplayStore>>;

/// The guard of the modes that verify RFC 9421 proofs of possession.
pub struct PopEdgeGuard {
    verifier: EdgePopVerifier,
    authority: EdgeAuthority,
    policy: ProofPolicy,
}

impl PopEdgeGuard {
    #[must_use]
    pub fn new(verifier: EdgePopVerifier, authority: EdgeAuthority, policy: ProofPolicy) -> Self {
        Self {
            verifier,
            authority,
            policy,
        }
    }

    #[must_use]
    pub fn policy(&self) -> ProofPolicy {
        self.policy
    }

    /// Reads the bearer's caller before the verifier records the request's
    /// `jti`, so a request naming nobody never reaches the replay store.
    async fn verify(
        &self,
        request: &InboundRequest,
        caller: &CallerHttpsAuth,
    ) -> Result<VerifiedEdgeCaller, EdgeAuthError> {
        let bearer = caller.bearer_subject().map_err(EdgeAuthError::UnidentifiedBearer)?;
        let presenter = self
            .verifier
            .verify(&signed_request(request, &self.authority))
            .await
            .map_err(EdgeAuthError::from_pop)?;
        let verified = VerifiedEdgeCaller::from_presenter(&presenter).map_err(EdgeAuthError::UnidentifiedPresenter)?;
        if verified.subject() != &bearer {
            return Err(EdgeAuthError::BearerMismatch {
                verified: verified.subject().clone(),
                bearer,
            });
        }
        Ok(verified)
    }
}

impl fmt::Debug for PopEdgeGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopEdgeGuard")
            .field("authority", &self.authority)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl EdgeGuard for PopEdgeGuard {
    async fn admit(&self, request: &InboundRequest, caller: &CallerHttpsAuth) -> Result<EdgeAdmission, EdgeAuthError> {
        match (self.verify(request, caller).await, self.policy) {
            (Ok(verified), _) => Ok(EdgeAdmission::Verified(verified)),
            (Err(error), ProofPolicy::Shadow) if !error.leaves_request_anonymous() => {
                Ok(EdgeAdmission::Shadowed(error))
            }
            (Err(error), _) => Err(error),
        }
    }

    fn mode(&self) -> EdgeAuthMode {
        self.policy.mode()
    }
}

/// The request as the verifier sees it: signed for the edge's configured
/// authority rather than whatever `Host` the request claims, with every
/// header value kept separate so a repeated signature header is refused.
fn signed_request(request: &InboundRequest, authority: &EdgeAuthority) -> HttpRequest {
    HttpRequest {
        method: request.method().as_str().to_owned(),
        authority: authority.as_str().to_owned(),
        path: request.path().to_owned(),
        headers: request
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body: Some(request.body().to_vec()),
    }
}

#[cfg(test)]
mod tests;
