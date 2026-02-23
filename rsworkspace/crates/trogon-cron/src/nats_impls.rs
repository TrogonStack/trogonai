use async_nats::jetstream::{self, kv};
use bytes::Bytes;

use crate::{error::CronError, kv::LEADER_KEY, traits::{LeaderLock, TickPublisher}};

/// Concrete `TickPublisher` backed by a JetStream context.
///
/// Ticks are published into the `CRON_TICKS` stream (must exist before calling),
/// giving workers at-least-once delivery: a worker that was briefly down will
/// receive missed ticks when it reconnects via a durable consumer.
impl TickPublisher for jetstream::Context {
    type Error = CronError;

    async fn publish_tick(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<(), CronError> {
        self.publish_with_headers(subject, headers, payload)
            .await
            .map_err(|e| CronError::Publish(e.to_string()))?
            .await
            .map_err(|e| CronError::Publish(e.to_string()))?;
        Ok(())
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
