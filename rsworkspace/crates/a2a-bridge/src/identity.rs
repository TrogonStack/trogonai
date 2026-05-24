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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallerHttpsAuth(String);

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
        f.pad(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeUserJwt(String);

impl BridgeUserJwt {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
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
