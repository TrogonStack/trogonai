use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::StsError;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentRegistryRecord {
    pub agent_id: String,
    pub agent_version: String,
    pub agent_definition_digest: String,
    pub owner_team: String,
    pub allowed_workloads: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_audiences: Vec<String>,
    pub allowed_purposes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_token_ttl_s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub lifecycle_state: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryLookupRequest {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RegistryLookupResponse {
    Ok {
        record: Box<AgentRegistryRecord>,
        resolved_version: String,
        kv_revision: u64,
    },
    NotFound {
        agent_id: String,
        reason: String,
    },
    Revoked {
        agent_id: String,
        reason: String,
    },
    Deprecated {
        agent_id: String,
        reason: String,
    },
    Error {
        reason: String,
    },
}

#[async_trait]
pub trait RegistryLookup: Send + Sync + Clone {
    async fn lookup(&self, request: &RegistryLookupRequest) -> Result<AgentRegistryRecord, StsError>;
}

#[derive(Clone)]
pub struct InMemoryRegistry {
    records: std::collections::HashMap<String, AgentRegistryRecord>,
}

impl InMemoryRegistry {
    pub fn new(records: impl IntoIterator<Item = AgentRegistryRecord>) -> Self {
        let mut map = std::collections::HashMap::new();
        for record in records {
            map.insert(record.agent_id.clone(), record);
        }
        Self { records: map }
    }
}

#[async_trait]
impl RegistryLookup for InMemoryRegistry {
    async fn lookup(&self, request: &RegistryLookupRequest) -> Result<AgentRegistryRecord, StsError> {
        let Some(record) = self.records.get(&request.agent_id) else {
            return Err(StsError::RegistryUnavailable(format!(
                "agent {} not found",
                request.agent_id
            )));
        };
        if record.lifecycle_state == "revoked" {
            return Err(StsError::AccessDenied(format!("agent {} is revoked", request.agent_id)));
        }
        Ok(record.clone())
    }
}

#[derive(Clone)]
pub struct NatsRegistryClient<C> {
    client: C,
    subject: String,
}

impl<C> NatsRegistryClient<C> {
    pub fn new(client: C, subject: impl Into<String>) -> Self {
        Self {
            client,
            subject: subject.into(),
        }
    }
}

#[async_trait]
impl<C> RegistryLookup for NatsRegistryClient<C>
where
    C: trogon_nats::client::RequestClient + Send + Sync + Clone,
{
    async fn lookup(&self, request: &RegistryLookupRequest) -> Result<AgentRegistryRecord, StsError> {
        let response: RegistryLookupResponse = trogon_nats::messaging::request_with_timeout(
            &self.client,
            &self.subject,
            request,
            std::time::Duration::from_secs(5),
        )
        .await
        .map_err(|e| StsError::RegistryUnavailable(format!("registry request failed: {e}")))?;

        match response {
            RegistryLookupResponse::Ok { record, .. } => Ok(*record),
            RegistryLookupResponse::NotFound { agent_id, reason } => Err(StsError::AccessDenied(format!(
                "registry not_found for {agent_id}: {reason}"
            ))),
            RegistryLookupResponse::Revoked { agent_id, reason } => Err(StsError::AccessDenied(format!(
                "registry revoked for {agent_id}: {reason}"
            ))),
            RegistryLookupResponse::Deprecated { agent_id, reason } => Err(StsError::AccessDenied(format!(
                "registry deprecated for {agent_id}: {reason}"
            ))),
            RegistryLookupResponse::Error { reason } => Err(StsError::RegistryUnavailable(reason)),
        }
    }
}
