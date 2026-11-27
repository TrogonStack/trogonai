use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt as _;

use super::entry::Entry;
use super::operation_store::OperationStore;
use super::operation_store_error::OperationStoreError;

pub struct NatsKvOperationStore {
    kv: kv::Store,
}

impl NatsKvOperationStore {
    /// Opens the bucket, creating it when missing. History 1, no max age:
    /// records are dropped by the sweeper, not by the server.
    pub async fn new(
        jetstream: jetstream::Context,
        bucket: &str,
        replicas: usize,
    ) -> Result<Self, OperationStoreError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                num_replicas: replicas,
                ..Default::default()
            })
            .await
            .map_err(OperationStoreError::backend)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl OperationStore for NatsKvOperationStore {
    async fn get(&self, key: &str) -> Result<Option<Entry>, OperationStoreError> {
        let Some(entry) = self.kv.entry(key).await.map_err(OperationStoreError::backend)? else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }
        Ok(Some(Entry {
            value: entry.value,
            revision: entry.revision,
        }))
    }

    async fn create(&self, key: &str, value: Bytes) -> Result<Option<u64>, OperationStoreError> {
        match self.kv.create(key, value).await {
            Ok(revision) => Ok(Some(revision)),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(OperationStoreError::backend(error)),
        }
    }

    async fn update(&self, key: &str, value: Bytes, revision: u64) -> Result<Option<u64>, OperationStoreError> {
        match self.kv.update(key, value, revision).await {
            Ok(revision) => Ok(Some(revision)),
            Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(None),
            Err(error) => Err(OperationStoreError::backend(error)),
        }
    }

    async fn delete(&self, key: &str, revision: Option<u64>) -> Result<bool, OperationStoreError> {
        if self.get(key).await?.is_none() {
            return Ok(false);
        }
        match self.kv.delete_expect_revision(key, revision).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == kv::DeleteErrorKind::WrongLastRevision => Ok(false),
            Err(error) => Err(OperationStoreError::backend(error)),
        }
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, OperationStoreError> {
        let mut keys: Vec<String> = self
            .kv
            .keys()
            .await
            .map_err(OperationStoreError::backend)?
            .try_filter(|key| std::future::ready(key.starts_with(prefix)))
            .try_collect()
            .await
            .map_err(OperationStoreError::backend)?;
        keys.sort();
        Ok(keys)
    }
}
