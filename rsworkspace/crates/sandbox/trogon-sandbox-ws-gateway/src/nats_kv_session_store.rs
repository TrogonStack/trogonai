use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;

use crate::constants::SESSION_BUCKET;
use crate::{SessionRecord, SessionStore, SessionStoreError};

/// A [`SessionStore`] backed by a JetStream key-value bucket whose entries expire after the session lifetime.
pub struct NatsKvSessionStore {
    kv: kv::Store,
}

impl NatsKvSessionStore {
    pub async fn new(jetstream: jetstream::Context, max_age: Duration) -> Result<Self, SessionStoreError> {
        let config = kv::Config {
            bucket: SESSION_BUCKET.to_string(),
            max_age: max_age.max(Duration::from_secs(1)),
            ..Default::default()
        };
        let kv = jetstream
            .create_or_update_key_value(config)
            .await
            .map_err(SessionStoreError::backend)?;
        Ok(Self { kv })
    }
}

#[async_trait]
impl SessionStore for NatsKvSessionStore {
    async fn put(&self, token_hash: &str, record: SessionRecord) -> Result<(), SessionStoreError> {
        let bytes = Bytes::from(serde_json::to_vec(&record)?);
        self.kv
            .put(token_hash, bytes)
            .await
            .map_err(SessionStoreError::backend)?;
        Ok(())
    }

    async fn lookup(&self, token_hash: &str) -> Result<Option<SessionRecord>, SessionStoreError> {
        let entry = self.kv.get(token_hash).await.map_err(SessionStoreError::backend)?;
        let Some(bytes) = entry else {
            return Ok(None);
        };
        let record: SessionRecord = serde_json::from_slice(&bytes)?;
        if record.is_expired() {
            self.kv.delete(token_hash).await.map_err(SessionStoreError::backend)?;
            return Ok(None);
        }
        Ok(Some(record))
    }
}
