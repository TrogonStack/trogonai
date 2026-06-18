//! Catalog registrar subject + payload decoding helpers.
//!
//! The async `run` loop (subscribe, dispatch, reply) lands in the integration PR
//! alongside its smoke harness so it can be exercised end-to-end with a NATS
//! mock; this slice ships the pure helpers (subject naming, payload parsing,
//! reply serialisation, JSON-RPC error mapping) that the loop is built from.

use bytes::Bytes;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::{A2aAgentId, AgentIdError};

use super::store::CatalogStoreError;

pub struct RegistrarSubject {
    prefix: A2aPrefix,
}

impl RegistrarSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }

    pub fn wildcard(&self) -> String {
        format!("{}.catalog.register.*", self.prefix.as_str())
    }

    pub fn for_agent(&self, agent_id: &A2aAgentId) -> String {
        format!("{}.catalog.register.{}", self.prefix.as_str(), agent_id.as_str())
    }
}

impl std::fmt::Display for RegistrarSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.catalog.register.*", self.prefix.as_str())
    }
}

pub fn register_subject_prefix(prefix: &A2aPrefix) -> String {
    format!("{}.catalog.register.", prefix.as_str())
}

/// Why a wire subject failed `{prefix}.catalog.register.{agent_id}` parsing.
#[derive(Debug, thiserror::Error)]
pub enum AgentSuffixError {
    /// The subject didn't carry the expected `{prefix}.catalog.register.` leader.
    #[error("subject is not a `{{prefix}}.catalog.register.` register subject")]
    NotARegisterSubject,
    /// Subject had the right leader but no agent-id token after the dot.
    #[error("register subject is missing the `{{agent_id}}` segment")]
    MissingAgentId,
    /// Agent-id token failed `A2aAgentId` validation.
    #[error("register subject agent_id is invalid: {0}")]
    InvalidAgentId(#[source] AgentIdError),
}

/// Extract a validated `A2aAgentId` from a `{prefix}.catalog.register.{agent_id}` subject.
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
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "result": null
    });
    serde_json::to_vec(&body).ok().map(Bytes::from)
}

pub fn error_reply(code: i32, message: &str) -> Option<Bytes> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": code, "message": message }
    });
    serde_json::to_vec(&body).ok().map(Bytes::from)
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
mod tests {
    use super::*;
    use crate::a2a_prefix::A2aPrefix;
    use crate::agent_id::A2aAgentId;

    fn prefix(s: &str) -> A2aPrefix {
        A2aPrefix::new(s).unwrap()
    }

    fn agent_id(s: &str) -> A2aAgentId {
        A2aAgentId::new(s).unwrap()
    }

    fn card(name: &str) -> a2a::agent_card::AgentCard {
        a2a::agent_card::AgentCard {
            name: name.to_string(),
            description: String::new(),
            version: String::new(),
            supported_interfaces: vec![a2a::agent_card::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: None,
            }],
            capabilities: a2a::agent_card::AgentCapabilities::default(),
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        }
    }

    #[test]
    fn registrar_subject_wildcard() {
        let sub = RegistrarSubject::new(&prefix("a2a"));
        assert_eq!(sub.wildcard(), "a2a.catalog.register.*");
    }

    #[test]
    fn registrar_subject_for_agent() {
        let sub = RegistrarSubject::new(&prefix("a2a"));
        assert_eq!(sub.for_agent(&agent_id("planner")), "a2a.catalog.register.planner");
    }

    #[test]
    fn registrar_subject_dotted_prefix() {
        let sub = RegistrarSubject::new(&prefix("my.multi.part"));
        assert_eq!(sub.wildcard(), "my.multi.part.catalog.register.*");
        assert_eq!(sub.for_agent(&agent_id("bot")), "my.multi.part.catalog.register.bot");
    }

    #[test]
    fn registrar_subject_display() {
        let sub = RegistrarSubject::new(&prefix("myapp"));
        assert_eq!(sub.to_string(), "myapp.catalog.register.*");
    }

    #[test]
    fn register_subject_prefix_takes_typed_prefix() {
        assert_eq!(register_subject_prefix(&prefix("a2a")), "a2a.catalog.register.");
        assert_eq!(register_subject_prefix(&prefix("my.app")), "my.app.catalog.register.");
    }

    #[test]
    fn agent_id_from_subject_returns_typed_agent_id() {
        let id = agent_id_from_subject("a2a.catalog.register.planner", &prefix("a2a")).unwrap();
        assert_eq!(id.as_str(), "planner");
    }

    #[test]
    fn agent_id_from_subject_rejects_subject_missing_register_leader() {
        let err = agent_id_from_subject("a2a.other.register.planner", &prefix("a2a")).unwrap_err();
        assert!(matches!(err, AgentSuffixError::NotARegisterSubject));
    }

    #[test]
    fn agent_id_from_subject_rejects_missing_agent_id_segment() {
        let err = agent_id_from_subject("a2a.catalog.register.", &prefix("a2a")).unwrap_err();
        assert!(matches!(err, AgentSuffixError::MissingAgentId));
    }

    #[test]
    fn agent_id_from_subject_rejects_invalid_agent_id_segment() {
        let err = agent_id_from_subject("a2a.catalog.register.bad*agent", &prefix("a2a")).unwrap_err();
        assert!(matches!(err, AgentSuffixError::InvalidAgentId(_)));
    }

    #[test]
    fn agent_suffix_error_display_and_source() {
        use std::error::Error;
        assert!(
            AgentSuffixError::NotARegisterSubject
                .to_string()
                .contains("register subject")
        );
        assert!(
            AgentSuffixError::MissingAgentId
                .to_string()
                .contains("missing the `{agent_id}`")
        );
        let id_err = A2aAgentId::new("bad*agent").unwrap_err();
        let wrap = AgentSuffixError::InvalidAgentId(id_err);
        assert!(wrap.to_string().contains("agent_id is invalid"));
        assert!(wrap.source().is_some());
        assert!(AgentSuffixError::MissingAgentId.source().is_none());
    }

    #[test]
    fn parse_register_payload_accepts_valid_card() {
        let payload = serde_json::to_vec(&card("bot")).unwrap();
        let parsed = parse_register_payload(&payload).unwrap();
        assert_eq!(parsed.name, "bot");
    }

    #[test]
    fn parse_register_payload_rejects_malformed_json() {
        let err = parse_register_payload(b"not-json").unwrap_err();
        let (code, message) = err.json_rpc();
        assert_eq!(code, -32700);
        assert!(message.starts_with("Parse error:"));
    }

    #[test]
    fn parse_register_payload_rejects_schema_violation() {
        let err = parse_register_payload(b"{}").unwrap_err();
        let (code, message) = err.json_rpc();
        assert_eq!(code, -32602);
        assert!(message.contains("JSON Schema"));
    }

    #[test]
    fn catalog_store_json_rpc_maps_kv_to_internal_error() {
        let inner: Box<dyn std::error::Error + Send + Sync> = Box::new(std::io::Error::other("conn failed"));
        let (code, message) = catalog_store_json_rpc(CatalogStoreError::Kv(inner));
        assert_eq!(code, -32603);
        assert!(message.contains("KV store error"));
    }

    #[test]
    fn catalog_store_json_rpc_maps_deserialize_to_parse_error() {
        let inner = serde_json::from_str::<String>("x").unwrap_err();
        let (code, _) = catalog_store_json_rpc(CatalogStoreError::Deserialize(inner));
        assert_eq!(code, -32700);
    }

    #[test]
    fn success_reply_serializes_null_result() {
        let bytes = success_reply().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert!(v["result"].is_null());
    }

    #[test]
    fn error_reply_serializes_correctly() {
        let bytes = error_reply(-32602, "invalid agent_id").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], -32602);
        assert_eq!(v["error"]["message"], "invalid agent_id");
    }

    #[test]
    fn register_payload_error_display_and_source_covers_every_variant() {
        use std::error::Error;
        let json = serde_json::from_str::<String>("x").unwrap_err();
        let schema = a2a_pack::validate_agent_card_value(&serde_json::json!({})).unwrap_err();
        let value = serde_json::from_str::<String>("x").unwrap_err();
        let json_err = RegisterPayloadError::JsonParse(json);
        let schema_err = RegisterPayloadError::Schema(schema);
        let value_err = RegisterPayloadError::ValueParse(value);
        assert!(json_err.to_string().contains("JSON parse"));
        assert!(schema_err.to_string().contains("schema validation"));
        assert!(value_err.to_string().contains("AgentCard parse"));
        assert!(json_err.source().is_some());
        assert!(schema_err.source().is_some());
        assert!(value_err.source().is_some());
    }

    #[test]
    fn catalog_store_json_rpc_maps_schema_to_invalid_params() {
        let schema_err = a2a_pack::validate_agent_card_value(&serde_json::json!({})).unwrap_err();
        let (code, _) = catalog_store_json_rpc(CatalogStoreError::AgentCardSchema(schema_err));
        assert_eq!(code, -32602);
    }

    #[test]
    fn parse_register_payload_value_parse_branch_maps_to_minus_32700() {
        // Schema accepts {name, supportedInterfaces} but AgentCard requires more fields,
        // so serde_json::from_value fails after the schema check passes.
        let payload = serde_json::to_vec(&serde_json::json!({
            "name": "bot",
            "supportedInterfaces": [{
                "url": "https://example.com/a2a",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "0.2.0"
            }]
        }))
        .unwrap();
        let err = parse_register_payload(&payload).unwrap_err();
        let (code, message) = err.json_rpc();
        assert_eq!(code, -32700);
        assert!(message.starts_with("Parse error:"));
    }
}
