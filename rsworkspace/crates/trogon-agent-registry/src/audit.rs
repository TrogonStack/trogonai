use async_nats::Client;
use serde::Serialize;
use tracing::warn;

use crate::types::AgentRecord;

pub const AUDIT_LOOKUP_FOUND: &str = "mcp.audit.registry.lookup.found";
pub const AUDIT_LOOKUP_NOTFOUND: &str = "mcp.audit.registry.lookup.notfound";
pub const AUDIT_LOOKUP_REVOKED: &str = "mcp.audit.registry.lookup.revoked";
pub const AUDIT_PUT: &str = "mcp.audit.registry.put";
pub const AUDIT_DELETE: &str = "mcp.audit.registry.delete";

#[derive(Debug, Serialize)]
pub struct LookupAuditEvent<'a> {
    pub agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_hint: Option<&'a str>,
    pub outcome: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MutationAuditEvent<'a> {
    pub event_type: &'static str,
    pub agent_id: &'a str,
    pub agent_version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kv_revision: Option<u64>,
}

pub async fn publish_lookup_audit(client: &Client, subject: &str, event: &LookupAuditEvent<'_>) {
    publish_json(client, subject, event).await;
}

pub async fn publish_put_audit(client: &Client, record: &AgentRecord, kv_revision: Option<u64>) {
    let event = MutationAuditEvent {
        event_type: "put",
        agent_id: &record.agent_id,
        agent_version: &record.agent_version,
        kv_revision,
    };
    publish_json(client, AUDIT_PUT, &event).await;
}

pub async fn publish_delete_audit(client: &Client, agent_id: &str, agent_version: Option<&str>) {
    let event = MutationAuditEvent {
        event_type: "delete",
        agent_id,
        agent_version: agent_version.unwrap_or("unknown"),
        kv_revision: None,
    };
    publish_json(client, AUDIT_DELETE, &event).await;
}

async fn publish_json(client: &Client, subject: &str, payload: &impl Serialize) {
    let body = match serde_json::to_vec(payload) {
        Ok(body) => body,
        Err(error) => {
            warn!(%error, subject, "registry audit serialization failed");
            return;
        }
    };
    if let Err(error) = client.publish(subject.to_string(), body.into()).await {
        warn!(%error, subject, "registry audit publish failed");
    }
}
