use std::fmt;

use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream::{self, kv};
use futures::StreamExt;
use trogon_nats::jetstream::is_create_key_value_already_exists;

use crate::audit::{publish_delete_audit, publish_put_audit};
use crate::cache::RegistryCache;
use crate::types::AgentRecord;

pub const BUCKET_NAME: &str = "mcp-agent-registry";

const MAX_VALUE_SIZE: i32 = 65_536;

#[derive(Debug)]
pub enum StoreError {
    BucketCreate(jetstream::context::CreateKeyValueError),
    BucketOpen(jetstream::context::KeyValueError),
    BucketMissing,
    KvGet(kv::EntryError),
    KvPut(kv::PutError),
    KvDelete(kv::DeleteError),
    KvKeys(kv::HistoryError),
    KvWatch(kv::WatchError),
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    InvalidKey(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BucketCreate(error) => write!(f, "failed to create KV bucket `{BUCKET_NAME}`: {error}"),
            Self::BucketOpen(error) => write!(f, "failed to open KV bucket `{BUCKET_NAME}`: {error}"),
            Self::BucketMissing => write!(
                f,
                "KV bucket `{BUCKET_NAME}` is missing (set TROGON_REGISTRY_AUTOCREATE=1 in dev)"
            ),
            Self::KvGet(error) => write!(f, "KV get failed: {error}"),
            Self::KvPut(error) => write!(f, "KV put failed: {error}"),
            Self::KvDelete(error) => write!(f, "KV delete failed: {error}"),
            Self::KvKeys(error) => write!(f, "KV keys failed: {error}"),
            Self::KvWatch(error) => write!(f, "KV watch failed: {error}"),
            Self::Serialize(error) => write!(f, "failed to serialize agent record: {error}"),
            Self::Deserialize(error) => write!(f, "failed to deserialize agent record: {error}"),
            Self::InvalidKey(message) => write!(f, "invalid registry key: {message}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BucketCreate(error) => Some(error),
            Self::BucketOpen(error) => Some(error),
            Self::KvGet(error) => Some(error),
            Self::KvPut(error) => Some(error),
            Self::KvDelete(error) => Some(error),
            Self::KvKeys(error) => Some(error),
            Self::KvWatch(error) => Some(error),
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::BucketMissing | Self::InvalidKey(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum RegistryWatchEvent {
    Put {
        agent_id: String,
        record: AgentRecord,
        revision: u64,
    },
    Delete {
        agent_id: String,
        revision: u64,
    },
}

pub fn bucket_config() -> kv::Config {
    kv::Config {
        bucket: BUCKET_NAME.to_owned(),
        history: 10,
        max_value_size: MAX_VALUE_SIZE,
        ..Default::default()
    }
}

pub fn latest_key(agent_id: &str) -> String {
    format!("{agent_id}/@latest")
}

pub fn agent_id_from_key(key: &str) -> Result<String, StoreError> {
    let Some((agent_id, suffix)) = key.rsplit_once('/') else {
        return Err(StoreError::InvalidKey(format!(
            "expected `{{agent_id}}/@latest`, got `{key}`"
        )));
    };
    if suffix != "@latest" {
        return Err(StoreError::InvalidKey(format!("expected @latest pointer, got `{key}`")));
    }
    Ok(agent_id.to_string())
}

pub async fn open_bucket(jetstream: &jetstream::Context, auto_create: bool) -> Result<kv::Store, StoreError> {
    if auto_create {
        match jetstream.create_key_value(bucket_config()).await {
            Ok(store) => Ok(store),
            Err(error) if is_create_key_value_already_exists(&error) => jetstream
                .get_key_value(BUCKET_NAME)
                .await
                .map_err(StoreError::BucketOpen),
            Err(error) => Err(StoreError::BucketCreate(error)),
        }
    } else {
        jetstream
            .get_key_value(BUCKET_NAME)
            .await
            .map_err(|_| StoreError::BucketMissing)
    }
}

#[derive(Clone)]
pub struct AgentRegistryStore {
    kv: kv::Store,
    nats: Client,
    prefix: Arc<str>,
}

impl AgentRegistryStore {
    pub fn new(kv: kv::Store, nats: Client, prefix: impl Into<Arc<str>>) -> Self {
        Self {
            kv,
            nats,
            prefix: prefix.into(),
        }
    }

    pub async fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        let key = latest_key(agent_id);
        let Some(entry) = self.kv.entry(key).await.map_err(StoreError::KvGet)? else {
            return Ok(None);
        };
        let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
        Ok(Some(record))
    }

    pub async fn put(&self, record: AgentRecord) -> Result<(), StoreError> {
        let key = latest_key(&record.agent_id);
        let payload = serde_json::to_vec(&record).map_err(StoreError::Serialize)?;
        let revision = self.kv.put(key, payload.into()).await.map_err(StoreError::KvPut)?;
        publish_put_audit(&self.nats, &self.prefix, &record, Some(revision)).await;
        Ok(())
    }

    pub async fn delete(&self, agent_id: &str) -> Result<(), StoreError> {
        let key = latest_key(agent_id);
        let version = self
            .get(agent_id)
            .await
            .ok()
            .flatten()
            .map(|record| record.agent_version);
        self.kv.delete(key).await.map_err(StoreError::KvDelete)?;
        publish_delete_audit(&self.nats, &self.prefix, agent_id, version.as_deref()).await;
        Ok(())
    }

    pub async fn watch(&self) -> Result<kv::Watch, StoreError> {
        self.kv.watch_all().await.map_err(StoreError::KvWatch)
    }

    pub async fn warm_cache(&self, cache: Arc<RegistryCache>) -> Result<(), StoreError> {
        let mut keys = self.kv.keys().await.map_err(StoreError::KvKeys)?;
        while let Some(key) = keys.next().await {
            let key = key.map_err(|error| StoreError::InvalidKey(error.to_string()))?;
            let Ok(agent_id) = agent_id_from_key(&key) else {
                continue;
            };
            if let Some(record) = self.get(&agent_id).await? {
                cache.insert(record).await;
            }
        }
        Ok(())
    }
}

pub fn map_watch_entry(entry: kv::Entry) -> Result<Option<RegistryWatchEvent>, StoreError> {
    let agent_id = match agent_id_from_key(&entry.key) {
        Ok(agent_id) => agent_id,
        Err(StoreError::InvalidKey(_)) => return Ok(None),
        Err(error) => return Err(error),
    };

    match entry.operation {
        kv::Operation::Put => {
            let record = serde_json::from_slice(&entry.value).map_err(StoreError::Deserialize)?;
            Ok(Some(RegistryWatchEvent::Put {
                agent_id,
                record,
                revision: entry.revision,
            }))
        }
        kv::Operation::Delete | kv::Operation::Purge => Ok(Some(RegistryWatchEvent::Delete {
            agent_id,
            revision: entry.revision,
        })),
    }
}

pub async fn spawn_watch_task(store: AgentRegistryStore, cache: Arc<RegistryCache>) -> Result<(), StoreError> {
    let mut watch = store.watch().await?;
    tokio::spawn(async move {
        while let Some(item) = watch.next().await {
            let entry = match item {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(%error, "registry KV watch error");
                    continue;
                }
            };
            match map_watch_entry(entry) {
                Ok(Some(RegistryWatchEvent::Put { record, .. })) => cache.insert(record).await,
                Ok(Some(RegistryWatchEvent::Delete { agent_id, .. })) => cache.remove(&agent_id).await,
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "registry KV watch entry ignored"),
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LifecycleState;

    #[test]
    fn latest_key_uses_pointer_suffix() {
        assert_eq!(latest_key("acme/oncall-agent"), "acme/oncall-agent/@latest");
    }

    #[test]
    fn agent_id_from_key_parses_latest_pointer() {
        assert_eq!(
            agent_id_from_key("acme/oncall-agent/@latest").expect("parse"),
            "acme/oncall-agent"
        );
    }

    #[test]
    fn agent_id_from_key_rejects_versioned_keys() {
        let error = agent_id_from_key("acme/oncall-agent/3.2.1").expect_err("must reject");
        assert!(matches!(error, StoreError::InvalidKey(_)));
    }

    #[test]
    fn map_watch_entry_put_round_trip() {
        let record = AgentRecord {
            agent_id: "acme/bot".to_string(),
            agent_version: "1.0.0".to_string(),
            agent_definition_digest: "sha256:deadbeef".to_string(),
            owner_team: "platform".to_string(),
            allowed_workloads: vec![],
            allowed_tools: vec![],
            allowed_audiences: vec![],
            allowed_purposes: None,
            mesh_token_ttl_s: None,
            metadata: serde_json::Value::Null,
            lifecycle_state: LifecycleState::Active,
            created_at: "2026-05-27T00:00:00Z".to_string(),
            updated_at: "2026-05-27T00:00:00Z".to_string(),
        };
        let body = serde_json::to_vec(&record).expect("serialize");
        let entry = kv::Entry {
            bucket: BUCKET_NAME.to_string(),
            key: latest_key("acme/bot"),
            value: bytes::Bytes::from(body),
            revision: 3,
            delta: 0,
            created: time::OffsetDateTime::UNIX_EPOCH,
            operation: kv::Operation::Put,
            seen_current: true,
        };
        let event = map_watch_entry(entry).expect("map").expect("some");
        assert_eq!(
            event,
            RegistryWatchEvent::Put {
                agent_id: "acme/bot".to_string(),
                record,
                revision: 3,
            }
        );
    }
}
