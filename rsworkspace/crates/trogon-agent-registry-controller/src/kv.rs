use std::fmt;

use async_nats::Client;
use trogon_agent_registry::{AgentRecord, latest_key, open_bucket};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub version_revision: u64,
    pub latest_revision: u64,
}

#[derive(Debug)]
pub enum KvError {
    Open(String),
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    Put(async_nats::jetstream::kv::PutError),
    Get(async_nats::jetstream::kv::EntryError),
    Connect(String),
}

impl fmt::Display for KvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(message) => write!(f, "open registry KV bucket: {message}"),
            Self::Serialize(error) => write!(f, "serialize agent record: {error}"),
            Self::Deserialize(error) => write!(f, "deserialize agent record: {error}"),
            Self::Put(error) => write!(f, "KV put failed: {error}"),
            Self::Get(error) => write!(f, "KV get failed: {error}"),
            Self::Connect(message) => write!(f, "NATS connect failed: {message}"),
        }
    }
}

impl std::error::Error for KvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::Put(error) => Some(error),
            Self::Get(error) => Some(error),
            Self::Open(_) | Self::Connect(_) => None,
        }
    }
}

pub struct ControllerStore {
    kv: async_nats::jetstream::kv::Store,
}

impl ControllerStore {
    pub async fn open(jetstream: &async_nats::jetstream::Context, auto_create: bool) -> Result<Self, KvError> {
        let kv = open_bucket(jetstream, auto_create)
            .await
            .map_err(|error| KvError::Open(error.to_string()))?;
        Ok(Self { kv })
    }

    pub async fn get_latest(&self, agent_id: &str) -> Result<Option<AgentRecord>, KvError> {
        let key = latest_key(agent_id);
        let Some(entry) = self.kv.entry(key).await.map_err(KvError::Get)? else {
            return Ok(None);
        };
        let record = serde_json::from_slice(&entry.value).map_err(KvError::Deserialize)?;
        Ok(Some(record))
    }

    pub async fn put_record(&self, record: AgentRecord) -> Result<SyncOutcome, KvError> {
        let version_key = format!("{}/{}", record.agent_id, record.agent_version);
        let payload = serde_json::to_vec(&record).map_err(KvError::Serialize)?;
        let version_revision = self
            .kv
            .put(version_key, payload.clone().into())
            .await
            .map_err(KvError::Put)?;
        let latest_revision = self
            .kv
            .put(latest_key(&record.agent_id), payload.into())
            .await
            .map_err(KvError::Put)?;
        Ok(SyncOutcome {
            version_revision,
            latest_revision,
        })
    }
}

pub async fn connect_nats(url: &str) -> Result<Client, KvError> {
    trogon_nats::connect(
        &trogon_nats::NatsConfig::new(vec![url.to_string()], trogon_nats::NatsAuth::None),
        std::time::Duration::from_secs(15),
    )
    .await
    .map_err(|error| KvError::Connect(error.to_string()))
}
