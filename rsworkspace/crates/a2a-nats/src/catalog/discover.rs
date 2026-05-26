use std::sync::Arc;

use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use async_nats::HeaderMap;
use bytes::Bytes;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::constants::GATEWAY_CALLER_ID_HEADER;
use crate::catalog::import_gate::SpiceDbPrincipal;
use crate::catalog::spicedb_permission::{
    AgentViewCheckOutcome, AgentViewGate, session_from_principal,
};

use super::store::CatalogStore;

pub struct DiscoverSubject {
    prefix: A2aPrefix,
}

impl DiscoverSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }

    pub fn wildcard(&self) -> String {
        format!("{}.discover.*", self.prefix.as_str())
    }

    pub fn for_agent(&self, agent_id: &A2aAgentId) -> String {
        format!("{}.discover.{}", self.prefix.as_str(), agent_id.as_str())
    }
}

impl std::fmt::Display for DiscoverSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.discover.*", self.prefix.as_str())
    }
}

fn success_reply(card: &a2a_types::AgentCard) -> Option<Bytes> {
    let value = serde_json::to_value(card).ok()?;
    if !accept_agent_card_on_read(&value, AgentCardSource::DiscoverResponse) {
        return None;
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "result": card
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

pub struct DiscoverService<S, N> {
    prefix: A2aPrefix,
    store: S,
    nats: N,
    view_gate: Arc<dyn AgentViewGate>,
    session_account: String,
}

impl<S, N> DiscoverService<S, N>
where
    S: CatalogStore,
    N: trogon_nats::SubscribeClient + trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    pub fn new(prefix: A2aPrefix, store: S, nats: N) -> Self {
        Self::with_view_gate(prefix, store, nats, Arc::new(crate::catalog::spicedb_permission::NoopAgentViewGate))
    }

    pub fn with_view_gate(
        prefix: A2aPrefix,
        store: S,
        nats: N,
        view_gate: Arc<dyn AgentViewGate>,
    ) -> Self {
        Self {
            prefix,
            store,
            nats,
            view_gate,
            session_account: "discover".into(),
        }
    }

    pub async fn run(self, shutdown: CancellationToken) -> Result<(), DiscoverServiceError> {
        let subject = DiscoverSubject::new(&self.prefix);
        let wildcard = subject.wildcard();
        let prefix_dot_discover = format!("{}.discover.", self.prefix.as_str());
        let prefix_len = prefix_dot_discover.len();

        let mut sub = self
            .nats
            .subscribe(async_nats::Subject::from(wildcard.as_str()))
            .await
            .map_err(|e| DiscoverServiceError::Subscribe(e.to_string()))?;

        info!(prefix = %self.prefix, "A2A discover service started");

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("A2A discover service shutting down");
                    break;
                }
                msg = sub.next() => {
                    match msg {
                        None => {
                            warn!("NATS discover subscription closed unexpectedly");
                            break;
                        }
                        Some(msg) => {
                            let Some(reply) = msg.reply.map(|s| s.to_string()) else {
                                warn!("discover request without reply subject; dropping");
                                continue;
                            };

                            let subject_str = msg.subject.as_str();
                            let agent_id_str = if subject_str.len() > prefix_len {
                                &subject_str[prefix_len..]
                            } else {
                                warn!("discover subject too short; dropping");
                                continue;
                            };

                            let agent_id = match A2aAgentId::new(agent_id_str) {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(error = %e, "invalid agent_id in discover subject; dropping");
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

                            if self.view_gate.is_enabled() {
                                let principal = discover_principal_from_headers(msg.headers.as_ref());
                                let Some(principal) = principal else {
                                    if let Some(b) = error_reply(-32001, &format!("agent not found: {agent_id}"))
                                        && let Err(e) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %e, "failed to publish discover authz-deny reply");
                                    }
                                    continue;
                                };
                                let Some(session) =
                                    session_from_principal(&principal, &self.session_account)
                                else {
                                    if let Some(b) = error_reply(-32001, &format!("agent not found: {agent_id}"))
                                        && let Err(e) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %e, "failed to publish discover session reply");
                                    }
                                    continue;
                                };
                                match self
                                    .view_gate
                                    .check_agent_view(&session, &principal, &agent_id)
                                    .await
                                {
                                    AgentViewCheckOutcome::Allowed => {}
                                    AgentViewCheckOutcome::Denied | AgentViewCheckOutcome::TransportError => {
                                        if let Some(b) = error_reply(-32001, &format!("agent not found: {agent_id}"))
                                            && let Err(e) = self.nats.publish_with_headers(
                                                async_nats::Subject::from(reply.as_str()),
                                                async_nats::HeaderMap::new(),
                                                b,
                                            ).await
                                        {
                                            warn!(error = %e, "failed to publish discover view-deny reply");
                                        }
                                        continue;
                                    }
                                }
                            }

                            match self.store.get_card(&agent_id).await {
                                Ok(Some(card)) => {
                                    if let Some(b) = success_reply(&card) {
                                        if let Err(e) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await {
                                            warn!(error = %e, "failed to publish discover reply");
                                        }
                                    } else {
                                        warn!(
                                            agent_id = %agent_id,
                                            source = AgentCardSource::DiscoverResponse.as_str(),
                                            "discover response dropped invalid AgentCard"
                                        );
                                        if let Some(b) = error_reply(-32603, "AgentCard failed read validation")
                                            && let Err(e) = self.nats.publish_with_headers(
                                                async_nats::Subject::from(reply.as_str()),
                                                async_nats::HeaderMap::new(),
                                                b,
                                            ).await
                                        {
                                            warn!(error = %e, "failed to publish discover validation error reply");
                                        }
                                    }
                                }
                                Ok(None) => {
                                    if let Some(b) = error_reply(-32001, &format!("agent not found: {agent_id}"))
                                        && let Err(e) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %e, "failed to publish discover not-found reply");
                                    }
                                }
                                Err(e) => {
                                    warn!(error = %e, "catalog store error during discover");
                                    if let Some(b) = error_reply(-32603, &e.to_string())
                                        && let Err(publish_err) = self.nats.publish_with_headers(
                                            async_nats::Subject::from(reply.as_str()),
                                            async_nats::HeaderMap::new(),
                                            b,
                                        ).await
                                    {
                                        warn!(error = %publish_err, "failed to publish discover error reply");
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
pub enum DiscoverServiceError {
    Subscribe(String),
}

impl std::fmt::Display for DiscoverServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(msg) => write!(f, "discover service subscribe failed: {msg}"),
        }
    }
}

impl std::error::Error for DiscoverServiceError {}

fn discover_principal_from_headers(headers: Option<&HeaderMap>) -> Option<SpiceDbPrincipal> {
    let headers = headers?;
    let caller = headers.get(GATEWAY_CALLER_ID_HEADER)?.as_str();
    let caller = caller.split('/').next_back().unwrap_or(caller);
    if caller.is_empty() {
        return None;
    }
    Some(SpiceDbPrincipal(serde_json::json!({
        "spicedb_subject": format!("user/{caller}"),
        "sub": caller,
    })))
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

    #[test]
    fn discover_subject_wildcard() {
        let sub = DiscoverSubject::new(&prefix("a2a"));
        assert_eq!(sub.wildcard(), "a2a.discover.*");
    }

    #[test]
    fn discover_subject_for_agent() {
        let sub = DiscoverSubject::new(&prefix("a2a"));
        assert_eq!(sub.for_agent(&agent_id("planner")), "a2a.discover.planner");
    }

    #[test]
    fn discover_subject_display() {
        let sub = DiscoverSubject::new(&prefix("myapp"));
        assert_eq!(sub.to_string(), "myapp.discover.*");
    }

    #[test]
    fn discover_service_error_display() {
        let e = DiscoverServiceError::Subscribe("no conn".into());
        assert!(e.to_string().contains("subscribe failed"));
    }

    #[test]
    fn discover_service_error_implements_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(DiscoverServiceError::Subscribe("x".into()));
        assert!(e.to_string().contains("x"));
    }

    fn minimal_valid_card(name: &str) -> a2a_types::AgentCard {
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
    fn success_reply_serializes_card() {
        let card = minimal_valid_card("bot");
        let bytes = success_reply(&card).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["result"]["name"], "bot");
    }

    #[test]
    fn success_reply_none_when_card_fails_read_validation() {
        let card = a2a_types::AgentCard {
            name: String::new(),
            ..Default::default()
        };
        assert!(success_reply(&card).is_none());
    }

    #[test]
    fn error_reply_serializes_correctly() {
        let bytes = error_reply(-32001, "not found").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], -32001);
        assert_eq!(v["error"]["message"], "not found");
    }
}
