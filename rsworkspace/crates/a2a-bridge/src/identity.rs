use std::fmt;

use a2a_auth_callout::CallerId;
use a2a_nats::{A2aAgentId, AgentIdError};

use crate::error::BridgeError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MintedCallerId(String);

impl MintedCallerId {
    #[must_use]
    pub fn from_caller_id(id: CallerId) -> Self {
        Self(id.as_str().to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CallerHttpsAuth(String);

impl fmt::Debug for CallerHttpsAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug must not leak the raw Authorization value either —
        // tracing/log macros use the `?` field syntax which routes
        // through Debug, not Display.
        f.debug_tuple("CallerHttpsAuth").field(&"<redacted>").finish()
    }
}

impl CallerHttpsAuth {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for CallerHttpsAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The wrapped value carries the raw `Authorization` header — Bearer
        // tokens, Basic credentials, etc. Redact in Display so accidental
        // tracing/log interpolation can't leak the secret. Callers that
        // genuinely need the value go through `as_str` / `into_inner`.
        f.pad("<redacted>")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BridgeUserJwt(String);

impl fmt::Debug for BridgeUserJwt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BridgeUserJwt").field(&"<redacted>").finish()
    }
}

impl BridgeUserJwt {
    /// Wrap a minted user JWT after validating compact-JWT shape. Mirrors
    /// the gate in `a2a_auth_callout::MintedUserJwt::new` so a malformed
    /// value can't be smuggled past this boundary and only fail later when
    /// the bridge tries to decode it.
    pub fn new(token: impl Into<String>) -> Result<Self, BridgeError> {
        let token = token.into().trim().to_owned();
        if token.is_empty() {
            return Err(BridgeError::Mint("minted user JWT must be non-empty".into()));
        }
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(BridgeError::Mint(
                "minted user JWT must be a compact 3-segment JWT".into(),
            ));
        }
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for BridgeUserJwt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("<redacted>")
    }
}

#[cfg(test)]
mod tests;
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeAgentId(A2aAgentId);

impl BridgeAgentId {
    pub fn parse(raw: &str) -> Result<Self, BridgeError> {
        A2aAgentId::new(raw)
            .map(Self)
            .map_err(|e: AgentIdError| BridgeError::InvalidAgent(e.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn as_agent_id(&self) -> &A2aAgentId {
        &self.0
    }

    #[must_use]
    pub fn into_agent_id(self) -> A2aAgentId {
        self.0
    }
}

impl fmt::Display for BridgeAgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
