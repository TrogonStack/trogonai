//! Pluggable JWKS resolver. The verifier resolves an issuer URL to a JwkSet so
//! tests can use in-memory keys and production can hit HTTP or NATS publishers.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;

/// Resolve an `iss` claim to the issuer's JWKS.
#[async_trait]
pub trait JwksResolver: Send + Sync {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError>;
}

#[derive(Debug, thiserror::Error)]
pub enum JwksError {
    #[error("unknown issuer: {0}")]
    UnknownIssuer(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("malformed JWKS: {0}")]
    Malformed(String),
}

/// In-memory map of `iss → JwkSet`. Used by tests and small static deployments.
#[derive(Clone, Default)]
pub struct StaticJwks {
    map: HashMap<String, JwkSet>,
}

impl StaticJwks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, iss: impl Into<String>, set: JwkSet) {
        self.map.insert(iss.into(), set);
    }
    #[must_use]
    pub fn with(mut self, iss: impl Into<String>, set: JwkSet) -> Self {
        self.insert(iss, set);
        self
    }
}

#[async_trait]
impl JwksResolver for StaticJwks {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        self.map
            .get(iss)
            .cloned()
            .ok_or_else(|| JwksError::UnknownIssuer(iss.to_string()))
    }
}

#[async_trait]
impl<R: JwksResolver + ?Sized> JwksResolver for Arc<R> {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        (**self).resolve(iss).await
    }
}
