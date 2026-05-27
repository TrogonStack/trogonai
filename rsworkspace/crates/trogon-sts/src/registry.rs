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

    async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        match self.lookup(request).await {
            Ok(record) => Ok(RegistryLookupResponse::Ok {
                record: Box::new(record),
                resolved_version: request.agent_id.clone(),
                kv_revision: 0,
            }),
            Err(StsError::AccessDenied(reason)) if reason.contains("not found") => {
                Ok(RegistryLookupResponse::NotFound {
                    agent_id: request.agent_id.clone(),
                    reason,
                })
            }
            Err(StsError::AccessDenied(reason)) if reason.contains("revoked") => {
                Ok(RegistryLookupResponse::Revoked {
                    agent_id: request.agent_id.clone(),
                    reason,
                })
            }
            Err(err) => Err(err),
        }
    }
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
        match self.lookup_raw(request).await? {
            RegistryLookupResponse::Ok { record, .. } => Ok(*record),
            RegistryLookupResponse::NotFound { agent_id, reason } => {
                Err(StsError::AccessDenied(format!("registry not_found for {agent_id}: {reason}")))
            }
            RegistryLookupResponse::Revoked { agent_id, reason } => {
                Err(StsError::AccessDenied(format!("registry revoked for {agent_id}: {reason}")))
            }
            RegistryLookupResponse::Deprecated { agent_id, reason } => {
                Err(StsError::AccessDenied(format!("registry deprecated for {agent_id}: {reason}")))
            }
            RegistryLookupResponse::Error { reason } => Err(StsError::RegistryUnavailable(reason)),
        }
    }

    async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        let Some(record) = self.records.get(&request.agent_id) else {
            return Ok(RegistryLookupResponse::NotFound {
                agent_id: request.agent_id.clone(),
                reason: format!("agent {} not found", request.agent_id),
            });
        };
        if record.lifecycle_state == "revoked" {
            return Ok(RegistryLookupResponse::Revoked {
                agent_id: request.agent_id.clone(),
                reason: format!("agent {} is revoked", request.agent_id),
            });
        }
        Ok(RegistryLookupResponse::Ok {
            record: Box::new(record.clone()),
            resolved_version: record.agent_version.clone(),
            kv_revision: 0,
        })
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
        match self.lookup_raw(request).await? {
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

    async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        trogon_nats::messaging::request_with_timeout(
            &self.client,
            &self.subject,
            request,
            std::time::Duration::from_secs(5),
        )
        .await
        .map_err(|e| StsError::RegistryUnavailable(format!("registry request failed: {e}")))
    }
}

#[derive(Clone)]
pub struct ResilientRegistry<R> {
    inner: R,
    breaker: crate::circuit_breaker::CircuitBreaker,
}

impl<R> ResilientRegistry<R> {
    pub fn new(inner: R, breaker: crate::circuit_breaker::CircuitBreaker) -> Self {
        Self { inner, breaker }
    }
}

#[async_trait]
impl<R> RegistryLookup for ResilientRegistry<R>
where
    R: RegistryLookup + Send + Sync,
{
    async fn lookup(&self, request: &RegistryLookupRequest) -> Result<AgentRegistryRecord, StsError> {
        map_breaker_error(
            self.breaker
                .call(|| async { self.inner.lookup(request).await })
                .await,
        )
    }

    async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        map_breaker_error(
            self.breaker
                .call(|| async { self.inner.lookup_raw(request).await })
                .await,
        )
    }
}

fn map_breaker_error<T>(result: Result<T, StsError>) -> Result<T, StsError> {
    match result {
        Err(StsError::DependencyUnavailable(reason)) => Err(StsError::RegistryUnavailable(reason)),
        other => other,
    }
}
