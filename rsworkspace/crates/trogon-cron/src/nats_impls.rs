use async_nats::jetstream::kv;
use bytes::Bytes;

use crate::{error::CronError, kv::LEADER_KEY, traits::{LeaderLock, TickPublisher}};

/// Concrete `TickPublisher` backed by an `async_nats::Client`.
impl TickPublisher for async_nats::Client {
    type Error = async_nats::client::PublishError;

    async fn publish_tick(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::Error> {
        self.publish_with_headers(subject, headers, payload).await
    }
}

/// Concrete `LeaderLock` backed by a NATS KV store.
#[derive(Clone)]
pub struct NatsLeaderLock {
    store: kv::Store,
}

impl NatsLeaderLock {
    pub fn new(store: kv::Store) -> Self {
        Self { store }
    }
}

impl LeaderLock for NatsLeaderLock {
    type Error = CronError;

    async fn try_acquire(&self, value: Bytes) -> Result<u64, CronError> {
        self.store
            .create(LEADER_KEY, value)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))
    }

    async fn renew(&self, value: Bytes, revision: u64) -> Result<u64, CronError> {
        self.store
            .update(LEADER_KEY, value, revision)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))
    }

    async fn release(&self) -> Result<(), CronError> {
        self.store
            .delete(LEADER_KEY)
            .await
            .map_err(|e| CronError::Kv(e.to_string()))
    }
}
