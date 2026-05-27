use std::fmt;

use std::sync::Arc;

use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use tracing::{debug, warn};

use crate::audit::{
    AUDIT_LOOKUP_FOUND, AUDIT_LOOKUP_NOTFOUND, AUDIT_LOOKUP_REVOKED, LookupAuditEvent, publish_lookup_audit,
};
use crate::cache::RegistryCache;
use crate::store::AgentRegistryStore;
use crate::types::{LifecycleState, LookupRequest, LookupResponse};

pub const LOOKUP_SUBJECT: &str = "mcp.registry.agent.lookup";
pub const QUEUE_GROUP: &str = "trogon-agent-registry";

#[derive(Debug)]
pub enum ConsumerError {
    Subscribe(async_nats::SubscribeError),
    RequestDeserialize(serde_json::Error),
    ResponseSerialize(serde_json::Error),
    Store(crate::store::StoreError),
}

impl fmt::Display for ConsumerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subscribe(error) => write!(f, "failed to subscribe to lookup subject: {error}"),
            Self::RequestDeserialize(error) => write!(f, "failed to decode lookup request: {error}"),
            Self::ResponseSerialize(error) => write!(f, "failed to encode lookup response: {error}"),
            Self::Store(error) => write!(f, "registry store error during lookup: {error}"),
        }
    }
}

impl std::error::Error for ConsumerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Subscribe(error) => Some(error),
            Self::RequestDeserialize(error) => Some(error),
            Self::ResponseSerialize(error) => Some(error),
            Self::Store(error) => Some(error),
        }
    }
}

pub fn resolve_lookup(request: &LookupRequest, record: Option<crate::types::AgentRecord>) -> LookupResponse {
    if let Some(hint) = request.tenant_hint.as_deref() {
        let expected_prefix = format!("{hint}/");
        if !request.agent_id.starts_with(&expected_prefix) {
            return LookupResponse::NotFound;
        }
    }

    let Some(record) = record else {
        return LookupResponse::NotFound;
    };

    if record.lifecycle_state == LifecycleState::Revoked {
        return LookupResponse::Revoked {
            reason: format!("agent `{}` is revoked", record.agent_id),
        };
    }

    LookupResponse::Found { record }
}

pub async fn lookup(
    store: &AgentRegistryStore,
    cache: Arc<RegistryCache>,
    request: &LookupRequest,
) -> Result<LookupResponse, ConsumerError> {
    let mut record = cache.get(&request.agent_id).await;
    if record.is_none() {
        record = store.get(&request.agent_id).await.map_err(ConsumerError::Store)?;
        if let Some(ref cached) = record {
            cache.insert(cached.clone()).await;
        }
    }
    Ok(resolve_lookup(request, record))
}

async fn publish_lookup_outcome(client: &Client, request: &LookupRequest, response: &LookupResponse) {
    let (subject, outcome) = match response {
        LookupResponse::Found { .. } => (AUDIT_LOOKUP_FOUND, "found"),
        LookupResponse::NotFound => (AUDIT_LOOKUP_NOTFOUND, "notfound"),
        LookupResponse::Revoked { .. } => (AUDIT_LOOKUP_REVOKED, "revoked"),
    };
    let event = LookupAuditEvent {
        agent_id: &request.agent_id,
        tenant_hint: request.tenant_hint.as_deref(),
        outcome,
    };
    publish_lookup_audit(client, subject, &event).await;
}

pub async fn run_lookup_consumer(
    client: Client,
    store: AgentRegistryStore,
    cache: Arc<RegistryCache>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), ConsumerError> {
    let mut subscription = client
        .queue_subscribe(LOOKUP_SUBJECT.to_string(), QUEUE_GROUP.to_string())
        .await
        .map_err(ConsumerError::Subscribe)?;

    debug!(
        subject = LOOKUP_SUBJECT,
        queue = QUEUE_GROUP,
        "registry lookup consumer started"
    );

    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        tokio::select! {
            _ = shutdown.as_mut() => break,
            message = subscription.next() => {
                let Some(message) = message else {
                    break;
                };
                if let Err(error) = handle_lookup_message(&client, &store, cache.clone(), message).await {
                    warn!(%error, "registry lookup request failed");
                }
            }
        }
    }

    Ok(())
}

async fn handle_lookup_message(
    client: &Client,
    store: &AgentRegistryStore,
    cache: Arc<RegistryCache>,
    message: async_nats::Message,
) -> Result<(), ConsumerError> {
    let Some(reply) = message.reply.clone() else {
        warn!("lookup request missing reply subject");
        return Ok(());
    };

    let request: LookupRequest = serde_json::from_slice(&message.payload).map_err(ConsumerError::RequestDeserialize)?;
    let response = lookup(store, cache, &request).await?;
    publish_lookup_outcome(client, &request, &response).await;

    let body = serde_json::to_vec(&response).map_err(ConsumerError::ResponseSerialize)?;
    if let Err(error) = client.publish(reply.to_string(), Bytes::from(body)).await {
        warn!(%error, "lookup reply publish failed");
        return Ok(());
    }
    client.flush().await.ok();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentRecord;

    fn sample_record(state: LifecycleState) -> AgentRecord {
        AgentRecord {
            agent_id: "acme/oncall-agent".to_string(),
            agent_version: "1.0.0".to_string(),
            agent_definition_digest: "sha256:abc".to_string(),
            owner_team: "platform".to_string(),
            allowed_workloads: vec![],
            allowed_tools: vec![],
            allowed_audiences: vec![],
            allowed_purposes: None,
            mesh_token_ttl_s: None,
            metadata: serde_json::Value::Null,
            lifecycle_state: state,
            created_at: "2026-05-27T00:00:00Z".to_string(),
            updated_at: "2026-05-27T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn resolve_lookup_returns_found_for_active_record() {
        let request = LookupRequest {
            agent_id: "acme/oncall-agent".to_string(),
            tenant_hint: None,
        };
        let record = sample_record(LifecycleState::Active);
        let response = resolve_lookup(&request, Some(record.clone()));
        assert_eq!(response, LookupResponse::Found { record });
    }

    #[test]
    fn resolve_lookup_returns_revoked_for_revoked_record() {
        let request = LookupRequest {
            agent_id: "acme/oncall-agent".to_string(),
            tenant_hint: None,
        };
        let response = resolve_lookup(&request, Some(sample_record(LifecycleState::Revoked)));
        assert!(matches!(response, LookupResponse::Revoked { .. }));
    }

    #[test]
    fn resolve_lookup_honors_tenant_hint_mismatch() {
        let request = LookupRequest {
            agent_id: "globex/agent".to_string(),
            tenant_hint: Some("acme".to_string()),
        };
        let response = resolve_lookup(&request, Some(sample_record(LifecycleState::Active)));
        assert_eq!(response, LookupResponse::NotFound);
    }
}
