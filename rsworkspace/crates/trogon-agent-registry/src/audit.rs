use async_nats::Client;
use serde::Serialize;
use tracing::warn;

use crate::types::AgentRecord;

pub fn audit_lookup_found_subject(prefix: &str) -> String {
    format!("{prefix}.audit.registry.lookup.found")
}
pub fn audit_lookup_notfound_subject(prefix: &str) -> String {
    format!("{prefix}.audit.registry.lookup.notfound")
}
pub fn audit_lookup_revoked_subject(prefix: &str) -> String {
    format!("{prefix}.audit.registry.lookup.revoked")
}
pub fn audit_put_subject(prefix: &str) -> String {
    format!("{prefix}.audit.registry.put")
}
pub fn audit_delete_subject(prefix: &str) -> String {
    format!("{prefix}.audit.registry.delete")
}

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

pub async fn publish_put_audit(client: &Client, prefix: &str, record: &AgentRecord, kv_revision: Option<u64>) {
    let event = MutationAuditEvent {
        event_type: "put",
        agent_id: &record.agent_id,
        agent_version: &record.agent_version,
        kv_revision,
    };
    publish_json(client, &audit_put_subject(prefix), &event).await;
}

pub async fn publish_delete_audit(client: &Client, prefix: &str, agent_id: &str, agent_version: Option<&str>) {
    let event = MutationAuditEvent {
        event_type: "delete",
        agent_id,
        agent_version: agent_version.unwrap_or("unknown"),
        kv_revision: None,
    };
    publish_json(client, &audit_delete_subject(prefix), &event).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_subjects_respect_custom_prefix() {
        assert_eq!(audit_put_subject("mcp"), "mcp.audit.registry.put");
        assert_eq!(audit_delete_subject("acme.mcp"), "acme.mcp.audit.registry.delete");
        assert_eq!(audit_lookup_found_subject("acme.mcp"), "acme.mcp.audit.registry.lookup.found");
        assert_eq!(audit_lookup_notfound_subject("mcp"), "mcp.audit.registry.lookup.notfound");
        assert_eq!(audit_lookup_revoked_subject("acme.mcp"), "acme.mcp.audit.registry.lookup.revoked");
    }
}
