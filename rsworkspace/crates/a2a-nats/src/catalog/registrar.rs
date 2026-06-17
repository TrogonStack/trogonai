//! Catalog registrar subject + payload decoding helpers.
//!
//! The async `run` loop (subscribe, dispatch, reply) lands in the integration PR
//! alongside its smoke harness so it can be exercised end-to-end with a NATS
//! mock; this slice ships the pure helpers (subject naming, payload parsing,
//! reply serialisation, JSON-RPC error mapping) that the loop is built from.

use bytes::Bytes;

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

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

pub fn register_subject_prefix(prefix: &str) -> String {
    format!("{prefix}.catalog.register.")
}

pub fn agent_id_suffix(subject: &str, prefix_len: usize) -> Option<&str> {
    subject.get(prefix_len..).filter(|s| !s.is_empty())
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

#[derive(Debug)]
pub enum RegisterPayloadError {
    JsonParse(serde_json::Error),
    Schema(a2a_pack::AgentCardValidateError),
    ValueParse(serde_json::Error),
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
    fn agent_id_suffix_peels_register_subject() {
        let prefix_dot = register_subject_prefix("a2a");
        let subject = "a2a.catalog.register.planner";
        assert_eq!(agent_id_suffix(subject, prefix_dot.len()), Some("planner"));
    }

    #[test]
    fn agent_id_suffix_returns_none_when_too_short() {
        let prefix_dot = register_subject_prefix("a2a");
        assert_eq!(agent_id_suffix("a2a.catalog.register.", prefix_dot.len()), None);
        assert_eq!(agent_id_suffix("a2a.catalog.register", prefix_dot.len()), None);
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
        let (code, message) = catalog_store_json_rpc(CatalogStoreError::Kv("conn failed".into()));
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
