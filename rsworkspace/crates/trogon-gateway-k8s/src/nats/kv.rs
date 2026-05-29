use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_nats::jetstream::kv::{self, Operation};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::DEFAULT_CONFIG_BUCKET;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKvPutOutcome {
    /// Payload unchanged — no KV write.
    Unchanged,
    /// New or updated value written.
    Written { revision: u64 },
}

#[derive(Debug, Clone)]
pub struct PutOptions {
    pub content_hash: String,
}

#[derive(Debug)]
pub enum ConfigKvError {
    Open(String),
    Get(String),
    Put(String),
    Delete(String),
    Connect(String),
}

impl fmt::Display for ConfigKvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(message) => write!(f, "open KV bucket: {message}"),
            Self::Get(message) => write!(f, "KV get failed: {message}"),
            Self::Put(message) => write!(f, "KV put failed: {message}"),
            Self::Delete(message) => write!(f, "KV delete failed: {message}"),
            Self::Connect(message) => write!(f, "NATS connect failed: {message}"),
        }
    }
}

impl std::error::Error for ConfigKvError {}

#[async_trait]
pub trait ConfigKv: Send + Sync {
    async fn put_idempotent(
        &self,
        key: &str,
        value: &[u8],
        options: PutOptions,
    ) -> Result<ConfigKvPutOutcome, ConfigKvError>;

    async fn delete_key(&self, key: &str) -> Result<(), ConfigKvError>;
}

pub struct NatsConfigKv {
    store: kv::Store,
}

impl NatsConfigKv {
    pub async fn connect(
        nats_url: &str,
        bucket: &str,
        creds_path: Option<&str>,
    ) -> Result<Self, ConfigKvError> {
        let auth = match creds_path {
            Some(path) => {
                trogon_nats::NatsAuth::Credentials(std::path::PathBuf::from(path))
            }
            None => trogon_nats::NatsAuth::None,
        };
        let client = trogon_nats::connect(
            &trogon_nats::NatsConfig::new(vec![nats_url.to_string()], auth),
            std::time::Duration::from_secs(15),
        )
        .await
        .map_err(|error| ConfigKvError::Connect(error.to_string()))?;

        let jetstream = async_nats::jetstream::new(client);
        let store = match jetstream.get_key_value(bucket).await {
            Ok(store) => store,
            Err(_) => jetstream
                .create_key_value(kv::Config {
                    bucket: bucket.to_string(),
                    ..Default::default()
                })
                .await
                .map_err(|error| ConfigKvError::Open(error.to_string()))?,
        };

        Ok(Self { store })
    }

    pub fn from_store(store: kv::Store) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ConfigKv for NatsConfigKv {
    async fn put_idempotent(
        &self,
        key: &str,
        value: &[u8],
        options: PutOptions,
    ) -> Result<ConfigKvPutOutcome, ConfigKvError> {
        if let Some(entry) = self
            .store
            .entry(key)
            .await
            .map_err(|error| ConfigKvError::Get(error.to_string()))?
        {
            if entry.operation != Operation::Delete
                && entry.operation != Operation::Purge
                && stored_hash_matches(&entry.value, &options.content_hash)
            {
                return Ok(ConfigKvPutOutcome::Unchanged);
            }
        }

        let revision = self
            .store
            .put(key, value.to_vec().into())
            .await
            .map_err(|error| ConfigKvError::Put(error.to_string()))?;
        Ok(ConfigKvPutOutcome::Written { revision })
    }

    async fn delete_key(&self, key: &str) -> Result<(), ConfigKvError> {
        self.store
            .delete(key)
            .await
            .map_err(|error| ConfigKvError::Delete(error.to_string()))
    }
}

#[derive(Default)]
struct MemoryEntry {
    value: Vec<u8>,
    content_hash: String,
    #[allow(dead_code)]
    revision: u64,
}

#[derive(Clone, Default)]
pub struct MemoryConfigKv {
    inner: Arc<RwLock<HashMap<String, MemoryEntry>>>,
    next_revision: Arc<RwLock<u64>>,
}

#[async_trait]
impl ConfigKv for MemoryConfigKv {
    async fn put_idempotent(
        &self,
        key: &str,
        value: &[u8],
        options: PutOptions,
    ) -> Result<ConfigKvPutOutcome, ConfigKvError> {
        let mut map = self.inner.write().await;
        if let Some(existing) = map.get(key)
            && existing.content_hash == options.content_hash
        {
            return Ok(ConfigKvPutOutcome::Unchanged);
        }
        let mut revision = self.next_revision.write().await;
        *revision += 1;
        let rev = *revision;
        map.insert(
            key.to_string(),
            MemoryEntry {
                value: value.to_vec(),
                content_hash: options.content_hash,
                revision: rev,
            },
        );
        Ok(ConfigKvPutOutcome::Written { revision: rev })
    }

    async fn delete_key(&self, key: &str) -> Result<(), ConfigKvError> {
        self.inner.write().await.remove(key);
        Ok(())
    }
}

impl MemoryConfigKv {
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner
            .read()
            .await
            .get(key)
            .map(|entry| entry.value.clone())
    }
}

fn stored_hash_matches(value: &[u8], expected: &str) -> bool {
    let actual = hex::encode(Sha256::digest(value));
    actual == expected
}

pub async fn open_default_config_kv(
    nats_url: &str,
    creds_path: Option<&str>,
) -> Result<NatsConfigKv, ConfigKvError> {
    NatsConfigKv::connect(nats_url, DEFAULT_CONFIG_BUCKET, creds_path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn idempotent_put_skips_identical_hash() {
        let kv = MemoryConfigKv::default();
        let payload = br#"{"tenant":"acme"}"#;
        let hash = hex::encode(Sha256::digest(payload));
        let options = PutOptions {
            content_hash: hash.clone(),
        };
        let first = kv
            .put_idempotent("mcp.gateway.config.tenant.edge", payload, options.clone())
            .await
            .expect("first put");
        assert!(matches!(first, ConfigKvPutOutcome::Written { .. }));

        let second = kv
            .put_idempotent("mcp.gateway.config.tenant.edge", payload, options)
            .await
            .expect("second put");
        assert_eq!(second, ConfigKvPutOutcome::Unchanged);
    }
}
