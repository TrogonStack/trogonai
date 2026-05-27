use async_nats::Client;
use async_trait::async_trait;
use serde::Deserialize;

use crate::constants::REGISTRY_LOOKUP_SUBJECT;
use crate::traits::Registry;
use crate::types::{AgentId, AgentRecord, SdkError};

pub struct NatsRegistry {
    client: Client,
    subject: String,
}

impl NatsRegistry {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            subject: REGISTRY_LOOKUP_SUBJECT.to_owned(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct LookupRequest<'a> {
    agent_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum LookupWireResponse {
    Found { record: RegistryRecordWire },
    NotFound,
    Revoked { reason: String },
}

#[derive(Debug, Deserialize)]
struct RegistryRecordWire {
    allowed_audiences: Vec<String>,
    #[serde(default)]
    allowed_purposes: Option<Vec<String>>,
    #[serde(default)]
    mesh_token_ttl_s: Option<u32>,
}

impl From<RegistryRecordWire> for AgentRecord {
    fn from(value: RegistryRecordWire) -> Self {
        Self {
            allowed_audiences: value.allowed_audiences,
            allowed_purposes: value.allowed_purposes.unwrap_or_default(),
            mesh_token_ttl_s: value.mesh_token_ttl_s.map(u64::from),
        }
    }
}

#[async_trait]
impl Registry for NatsRegistry {
    async fn lookup(&self, agent_id: &AgentId) -> Result<AgentRecord, SdkError> {
        let req = LookupRequest {
            agent_id: agent_id.as_str(),
        };
        let payload = serde_json::to_vec(&req).map_err(|e| SdkError::Serialization(e.to_string()))?;
        let response = self
            .client
            .request(self.subject.clone(), payload.into())
            .await
            .map_err(SdkError::nats)?;
        let body = response.payload;
        let wire: LookupWireResponse =
            serde_json::from_slice(&body).map_err(|e| SdkError::LookupFailed(e.to_string()))?;
        match wire {
            LookupWireResponse::Found { record } => Ok(record.into()),
            LookupWireResponse::NotFound => Err(SdkError::LookupFailed(format!(
                "agent {} not found",
                agent_id.as_str()
            ))),
            LookupWireResponse::Revoked { reason } => Err(SdkError::LookupFailed(reason)),
        }
    }
}
