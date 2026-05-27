use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical definition: `trogon-mcp-gateway::act_chain::ActChainEntry`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ActChainEntry {
    pub sub: String,
    pub agent_id: Option<String>,
    pub wkl: Option<String>,
    pub iat: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentIdError {
    #[error("agent id must be {{tenant}}/{{name}}")]
    InvalidFormat,
    #[error("agent id segments must be non-empty")]
    EmptySegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn parse(value: impl Into<String>) -> Result<Self, AgentIdError> {
        let value = value.into();
        let (tenant, name) = value.split_once('/').ok_or(AgentIdError::InvalidFormat)?;
        if tenant.is_empty() || name.is_empty() {
            return Err(AgentIdError::EmptySegment);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn tenant(&self) -> &str {
        self.0.split_once('/').expect("validated").0
    }

    pub fn name(&self) -> &str {
        self.0.split_once('/').expect("validated").1
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentId {
    type Err = AgentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Audience(String);

impl Audience {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn for_agent(agent: &AgentId) -> Self {
        Self(format!("urn:trogon:a2a:agent:{}:{}", agent.tenant(), agent.name()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Audience {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Purpose(String);

impl Purpose {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Purpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub originator: ActChainEntry,
    pub chain: Vec<ActChainEntry>,
    pub direct: ActChainEntry,
    pub purpose: Option<Purpose>,
    pub session_id: Option<String>,
}

impl Caller {
    pub fn chain_depth(&self) -> usize {
        self.chain.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentRecord {
    pub allowed_audiences: Vec<String>,
    #[serde(default)]
    pub mesh_token_ttl_s: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRequest {
    pub subject_token: String,
    pub actor_token: String,
    pub audience: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub token_type: Option<String>,
}

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("registry lookup failed: {0}")]
    LookupFailed(String),
    #[error("STS exchange failed: {0}")]
    ExchangeFailed(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("act chain exceeds max depth of {0}")]
    ChainTooDeep(usize),
    #[error("act chain contains a loop at ({agent_id}, {wkl})")]
    ChainLoop { agent_id: String, wkl: String },
    #[error("transport error")]
    Transport(#[source] async_nats::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("configuration error: {0}")]
    Config(String),
}

impl SdkError {
    pub fn nats<E>(err: async_nats::error::Error<E>) -> Self
    where
        E: std::fmt::Debug + std::fmt::Display + Clone + PartialEq + Send + Sync + 'static,
    {
        Self::Transport(Box::new(err))
    }

    pub fn transport_msg(message: impl Into<String>) -> Self {
        Self::Transport(Box::new(std::io::Error::other(message.into())))
    }
}
