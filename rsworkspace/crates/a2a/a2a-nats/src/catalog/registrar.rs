//! Catalog registrar subject + payload decoding helpers.
//!
//! The async `run` loop (subscribe, dispatch, reply) lands in the integration PR
//! alongside its smoke harness so it can be exercised end-to-end with a NATS
//! mock; this slice ships the pure helpers (subject naming, payload parsing,
//! reply serialisation, JSON-RPC error mapping) that the loop is built from.

use bytes::Bytes;
use jsonrpc_nats::ResponseId;
use serde_json::Value;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::{A2aAgentId, AgentIdError};
use crate::wire::{encode_error, encode_success};

use super::store::CatalogStoreError;

pub struct RegistrarSubject {
    prefix: A2aPrefix,
}

impl RegistrarSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }

    pub fn wildcard(&self) -> String {
        format!("{}.v1.catalog.register.*", self.prefix.as_str())
    }

    pub fn for_agent(&self, agent_id: &A2aAgentId) -> String {
        format!("{}.v1.catalog.register.{}", self.prefix.as_str(), agent_id.as_str())
    }
}

impl std::fmt::Display for RegistrarSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.catalog.register.*", self.prefix.as_str())
    }
}

pub fn register_subject_prefix(prefix: &A2aPrefix) -> String {
    format!("{}.v1.catalog.register.", prefix.as_str())
}

/// Why a wire subject failed `{prefix}.v1.catalog.register.{agent_id}` parsing.
#[derive(Debug, thiserror::Error)]
pub enum AgentSuffixError {
    /// The subject didn't carry the expected `{prefix}.v1.catalog.register.` leader.
    #[error("subject is not a `{{prefix}}.v1.catalog.register.` register subject")]
    NotARegisterSubject,
    /// Subject had the right leader but no agent-id token after the dot.
    #[error("register subject is missing the `{{agent_id}}` segment")]
    MissingAgentId,
    /// Agent-id token failed `A2aAgentId` validation.
    #[error("register subject agent_id is invalid: {0}")]
    InvalidAgentId(#[source] AgentIdError),
}

/// Extract a validated `A2aAgentId` from a `{prefix}.v1.catalog.register.{agent_id}` subject.
///
/// Returns a typed error instead of an `Option<&str>` so the bad-shape, missing
/// agent-id, and validation-failed paths each propagate distinctly to the caller.
pub fn agent_id_from_subject(subject: &str, prefix: &A2aPrefix) -> Result<A2aAgentId, AgentSuffixError> {
    let leader = register_subject_prefix(prefix);
    let remainder = subject
        .strip_prefix(leader.as_str())
        .ok_or(AgentSuffixError::NotARegisterSubject)?;
    if remainder.is_empty() {
        return Err(AgentSuffixError::MissingAgentId);
    }
    A2aAgentId::new(remainder).map_err(AgentSuffixError::InvalidAgentId)
}

pub fn success_reply() -> Option<Bytes> {
    encode_success(ResponseId::Null, &Value::Null)
        .ok()
        .map(|encoded| encoded.body)
}

pub fn error_reply(code: i32, message: &str) -> Option<Bytes> {
    Some(encode_error(ResponseId::Null, code, message, None).body)
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterPayloadError {
    #[error("JSON parse error: {0}")]
    JsonParse(#[source] serde_json::Error),
    #[error("AgentCard schema validation failed: {0}")]
    Schema(#[source] a2a_pack::AgentCardValidateError),
    #[error("AgentCard parse error: {0}")]
    ValueParse(#[source] serde_json::Error),
}

impl RegisterPayloadError {
    pub fn json_rpc(&self) -> (i32, String) {
        match self {
            Self::JsonParse(e) => (-32700, format!("Parse error: {e}")),
            Self::Schema(e) => (-32602, format!("AgentCard rejected by JSON Schema: {e}")),
            Self::ValueParse(e) => (-32700, format!("Parse error: {e}")),
        }
    }
}

pub fn parse_register_payload(payload: &[u8]) -> Result<a2a::agent_card::AgentCard, RegisterPayloadError> {
    let value: serde_json::Value = serde_json::from_slice(payload).map_err(RegisterPayloadError::JsonParse)?;
    a2a_pack::validate_agent_card_value(&value).map_err(RegisterPayloadError::Schema)?;
    serde_json::from_value::<a2a::agent_card::AgentCard>(value).map_err(RegisterPayloadError::ValueParse)
}

pub fn catalog_store_json_rpc(error: CatalogStoreError) -> (i32, String) {
    match error {
        CatalogStoreError::Deserialize(e) => (-32700, format!("Parse error: {e}")),
        CatalogStoreError::AgentCardSchema(e) => (-32602, format!("AgentCard rejected by JSON Schema: {e}")),
        other => (-32603, other.to_string()),
    }
}

#[cfg(test)]
mod tests;
