//! Person Server backing store. In-memory by default; the trait is the seam
//! for the JetStream-backed implementation.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRecord {
    pub agent_id: String,
    pub cnf_jwk: serde_json::Value,
    pub principal: Option<String>,
    pub iat: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRecord {
    pub principal: String,
    pub agent_id: String,
    pub resource_iss: String,
    pub scope: String,
    pub exp: i64,
}

#[async_trait]
pub trait PersonStore: Send + Sync {
    async fn put_agent(&self, agent: AgentRecord) -> Result<(), StoreError>;
    async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError>;
    async fn put_consent(&self, consent: ConsentRecord) -> Result<(), StoreError>;
    async fn get_consent(
        &self,
        principal: &str,
        agent_id: &str,
        resource_iss: &str,
    ) -> Result<Option<ConsentRecord>, StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("backend: {0}")]
    Backend(String),
}

pub struct InMemoryStore {
    agents: Mutex<HashMap<String, AgentRecord>>,
    consents: Mutex<HashMap<String, ConsentRecord>>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self {
            agents: Mutex::new(HashMap::new()),
            consents: Mutex::new(HashMap::new()),
        }
    }
}

fn consent_key(principal: &str, agent_id: &str, resource_iss: &str) -> String {
    format!("{principal}|{agent_id}|{resource_iss}")
}

#[async_trait]
impl PersonStore for InMemoryStore {
    async fn put_agent(&self, agent: AgentRecord) -> Result<(), StoreError> {
        self.agents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .insert(agent.agent_id.clone(), agent);
        Ok(())
    }

    async fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        Ok(self
            .agents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .get(agent_id)
            .cloned())
    }

    async fn put_consent(&self, consent: ConsentRecord) -> Result<(), StoreError> {
        let key = consent_key(&consent.principal, &consent.agent_id, &consent.resource_iss);
        self.consents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .insert(key, consent);
        Ok(())
    }

    async fn get_consent(
        &self,
        principal: &str,
        agent_id: &str,
        resource_iss: &str,
    ) -> Result<Option<ConsentRecord>, StoreError> {
        let key = consent_key(principal, agent_id, resource_iss);
        Ok(self
            .consents
            .lock()
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .get(&key)
            .cloned())
    }
}
