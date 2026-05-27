use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::AgentRecord;

#[derive(Debug, Default)]
pub struct RegistryCache {
    inner: RwLock<HashMap<String, AgentRecord>>,
}

impl RegistryCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(HashMap::new()),
        })
    }

    pub async fn get(&self, agent_id: &str) -> Option<AgentRecord> {
        self.inner.read().await.get(agent_id).cloned()
    }

    pub async fn insert(&self, record: AgentRecord) {
        self.inner.write().await.insert(record.agent_id.clone(), record);
    }

    pub async fn remove(&self, agent_id: &str) {
        self.inner.write().await.remove(agent_id);
    }
}
