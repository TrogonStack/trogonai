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
struct LookupOk {
    status: String,
    record: AgentRecord,
}

#[derive(Debug, Deserialize)]
struct LookupNack {
    status: String,
    reason: Option<String>,
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
        if let Ok(nack) = serde_json::from_slice::<LookupNack>(&body)
            && nack.status != "ok"
        {
            return Err(SdkError::LookupFailed(nack.reason.unwrap_or(nack.status)));
        }
        let ok: LookupOk = serde_json::from_slice(&body).map_err(|e| SdkError::LookupFailed(e.to_string()))?;
        if ok.status != "ok" {
            return Err(SdkError::LookupFailed(ok.status));
        }
        Ok(ok.record)
    }
}
