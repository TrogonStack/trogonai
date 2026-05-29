use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRecord {
    pub agent_id: String,
    pub lifecycle_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    NotFound,
    Revoked,
    Unavailable(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("agent not found in registry"),
            Self::Revoked => f.write_str("agent revoked in registry"),
            Self::Unavailable(reason) => write!(f, "registry unavailable: {reason}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[async_trait]
pub trait AgentRegistry: Send + Sync {
    async fn lookup(&self, agent_id: &str) -> Result<RegistryRecord, RegistryError>;
}

#[derive(Clone)]
pub struct MockAgentRegistry {
    records: HashMap<String, RegistryRecord>,
    lookups: Arc<Mutex<Vec<String>>>,
    unavailable: bool,
}

impl MockAgentRegistry {
    pub fn new(records: impl IntoIterator<Item = RegistryRecord>) -> Self {
        let mut map = HashMap::new();
        for record in records {
            map.insert(record.agent_id.clone(), record);
        }
        Self {
            records: map,
            lookups: Arc::new(Mutex::new(Vec::new())),
            unavailable: false,
        }
    }

    pub fn with_unavailable(mut self) -> Self {
        self.unavailable = true;
        self
    }

    pub fn lookup_count(&self) -> usize {
        self.lookups.lock().expect("mock registry lock").len()
    }

    pub fn last_lookup(&self) -> Option<String> {
        self.lookups.lock().expect("mock registry lock").last().cloned()
    }
}

#[async_trait]
impl AgentRegistry for MockAgentRegistry {
    async fn lookup(&self, agent_id: &str) -> Result<RegistryRecord, RegistryError> {
        self.lookups
            .lock()
            .expect("mock registry lock")
            .push(agent_id.to_string());
        if self.unavailable {
            return Err(RegistryError::Unavailable("mock unavailable".into()));
        }
        let Some(record) = self.records.get(agent_id) else {
            return Err(RegistryError::NotFound);
        };
        if record.lifecycle_state == "revoked" {
            return Err(RegistryError::Revoked);
        }
        Ok(record.clone())
    }
}
