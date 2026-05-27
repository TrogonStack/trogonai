use async_nats::Client;
use serde::Serialize;
use tracing::warn;

pub const AUDIT_REGISTERED: &str = "mcp.audit.registry.registered";
pub const AUDIT_BUMPED: &str = "mcp.audit.registry.bumped";
pub const AUDIT_DEPRECATED: &str = "mcp.audit.registry.deprecated";
pub const AUDIT_REVOKED: &str = "mcp.audit.registry.revoked";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryMutationKind {
    Registered,
    VersionBump,
    Deprecated,
    Revoked,
}

impl RegistryMutationKind {
    pub fn subject(self) -> &'static str {
        match self {
            Self::Registered => AUDIT_REGISTERED,
            Self::VersionBump => AUDIT_BUMPED,
            Self::Deprecated => AUDIT_DEPRECATED,
            Self::Revoked => AUDIT_REVOKED,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuditEvent<'a> {
    pub event_type: &'static str,
    pub agent_id: &'a str,
    pub agent_version: &'a str,
    pub prior_lifecycle_state: Option<&'static str>,
    pub new_lifecycle_state: &'static str,
    pub manifest_digest: &'a str,
    pub operator: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kv_revision: Option<u64>,
    pub git_commit: &'a str,
}

pub struct AuditPublisher {
    client: Client,
    operator: String,
}

impl AuditPublisher {
    pub fn new(client: Client, operator: impl Into<String>) -> Self {
        Self {
            client,
            operator: operator.into(),
        }
    }

    pub async fn emit(&self, kind: RegistryMutationKind, event: AuditEvent<'_>) {
        let subject = kind.subject();
        let body = match serde_json::to_vec(&event) {
            Ok(body) => body,
            Err(error) => {
                warn!(%error, subject, "registry controller audit serialization failed");
                return;
            }
        };
        if let Err(error) = self.client.publish(subject.to_string(), body.into()).await {
            warn!(%error, subject, "registry controller audit publish failed");
        }
    }

    pub fn operator(&self) -> &str {
        &self.operator
    }
}

pub fn lifecycle_label(state: trogon_agent_registry::LifecycleState) -> &'static str {
    match state {
        trogon_agent_registry::LifecycleState::Active => "active",
        trogon_agent_registry::LifecycleState::Deprecated => "deprecated",
        trogon_agent_registry::LifecycleState::Revoked => "revoked",
    }
}

pub fn classify_mutation(
    prior: Option<&trogon_agent_registry::AgentRecord>,
    next: &trogon_agent_registry::AgentRecord,
) -> RegistryMutationKind {
    match prior {
        None => RegistryMutationKind::Registered,
        Some(existing) if existing.agent_version != next.agent_version => RegistryMutationKind::VersionBump,
        Some(_) if next.lifecycle_state == trogon_agent_registry::LifecycleState::Revoked => {
            RegistryMutationKind::Revoked
        }
        Some(_) if next.lifecycle_state == trogon_agent_registry::LifecycleState::Deprecated => {
            RegistryMutationKind::Deprecated
        }
        Some(_) => RegistryMutationKind::VersionBump,
    }
}
