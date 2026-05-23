use bytes::Bytes;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;

use super::store::{CatalogStore, CatalogStoreError};

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

fn register_subject_prefix(prefix: &str) -> String {
    format!("{prefix}.catalog.register.")
}

fn agent_id_suffix(subject: &str, prefix_len: usize) -> Option<&str> {
    subject.get(prefix_len..).filter(|s| !s.is_empty())
}

fn success_reply() -> Option<Bytes> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "result": null
    });
    serde_json::to_vec(&body).ok().map(Bytes::from)
}

fn error_reply(code: i32, message: &str) -> Option<Bytes> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": { "code": code, "message": message }
    });
    serde_json::to_vec(&body).ok().map(Bytes::from)
}

#[derive(Debug)]
enum RegisterPayloadError {
    JsonParse(serde_json::Error),
    Schema(a2a_pack::AgentCardValidateError),
    ValueParse(serde_json::Error),
}

impl RegisterPayloadError {
    fn json_rpc(&self) -> (i32, String) {
        match self {
            Self::JsonParse(e) => (-32700, format!("Parse error: {e}")),
            Self::Schema(e) => (-32602, format!("AgentCard rejected by JSON Schema: {e}")),
            Self::ValueParse(e) => (-32700, format!("Parse error: {e}")),
        }
    }
}

fn parse_register_payload(payload: &[u8]) -> Result<a2a_types::AgentCard, RegisterPayloadError> {
    let value: serde_json::Value = serde_json::from_slice(payload).map_err(RegisterPayloadError::JsonParse)?;
    a2a_pack::validate_agent_card_value(&value).map_err(RegisterPayloadError::Schema)?;
    serde_json::from_value::<a2a_types::AgentCard>(value).map_err(RegisterPayloadError::ValueParse)
}

fn catalog_store_json_rpc(error: CatalogStoreError) -> (i32, String) {
    match error {
        CatalogStoreError::Deserialize(e) => (-32700, format!("Parse error: {e}")),
        CatalogStoreError::AgentCardSchema(e) => (-32602, format!("AgentCard rejected by JSON Schema: {e}")),
        other => (-32603, other.to_string()),
    }
}

pub struct CatalogRegistrarService<S, N> {
    prefix: A2aPrefix,
    store: S,
    nats: N,
}

impl<S, N> CatalogRegistrarService<S, N>
where
    S: CatalogStore,
    N: trogon_nats::SubscribeClient + trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    pub fn new(prefix: A2aPrefix, store: S, nats: N) -> Self {
        Self { prefix, store, nats }
    }

    pub async fn run(self, shutdown: CancellationToken) -> Result<(), CatalogRegistrarServiceError> {
        let subject = RegistrarSubject::new(&self.prefix);
        let wildcard = subject.wildcard();
        let prefix_dot_register = register_subject_prefix(self.prefix.as_str());
        let prefix_len = prefix_dot_register.len();

        let mut sub = self
            .nats
            .subscribe(async_nats::Subject::from(wildcard.as_str()))
            .await
            .map_err(|e| CatalogRegistrarServiceError::Subscribe(e.to_string()))?;

        info!(prefix = %self.prefix, "A2A catalog registrar service started");

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("A2A catalog registrar service shutting down");
                    break;
                }
                msg = sub.next() => {
                    match msg {
                        None => {
                            warn!("NATS catalog register subscription closed unexpectedly");
                            break;
                        }
                        Some(msg) => {
                            let Some(reply) = msg.reply.map(|s| s.to_string()) else {
                                warn!("catalog register request without reply subject; dropping");
                                continue;
                            };

                            let subject_str = msg.subject.as_str();
                            let Some(agent_id_str) = agent_id_suffix(subject_str, prefix_len) else {
                                warn!("catalog register subject too short; dropping");
                                continue;
                            };

                            let agent_id = match A2aAgentId::new(agent_id_str) {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(error = %e, "invalid agent_id in catalog register subject; dropping");
                                    if let Some(b) = error_reply(-32602, &format!("invalid agent_id: {e}")) {
                                        let _ = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await;
                                    }
                                    continue;
                                }
                            };

                            let card = match parse_register_payload(&msg.payload) {
                                Ok(card) => card,
                                Err(e) => {
                                    let (code, message) = e.json_rpc();
                                    warn!(error = %message, "invalid catalog register payload");
                                    if let Some(b) = error_reply(code, &message)
                                        && let Err(publish_err) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %publish_err, "failed to publish catalog register error reply");
                                    }
                                    continue;
                                }
                            };

                            match self.store.put_card(&agent_id, &card).await {
                                Ok(()) => {
                                    if let Some(b) = success_reply()
                                        && let Err(e) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %e, "failed to publish catalog register reply");
                                    }
                                }
                                Err(e) => {
                                    let (code, message) = catalog_store_json_rpc(e);
                                    warn!(error = %message, "catalog store error during register");
                                    if let Some(b) = error_reply(code, &message)
                                        && let Err(publish_err) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %publish_err, "failed to publish catalog register error reply");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum CatalogRegistrarServiceError {
    Subscribe(String),
}

impl std::fmt::Display for CatalogRegistrarServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(msg) => write!(f, "catalog registrar service subscribe failed: {msg}"),
        }
    }
}

impl std::error::Error for CatalogRegistrarServiceError {}

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

    fn card(name: &str) -> a2a_types::AgentCard {
        a2a_types::AgentCard {
            name: name.to_string(),
            supported_interfaces: vec![a2a_types::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: String::new(),
            }],
            ..Default::default()
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
    fn catalog_registrar_service_error_display() {
        let e = CatalogRegistrarServiceError::Subscribe("no conn".into());
        assert!(e.to_string().contains("subscribe failed"));
    }
}
