use std::time::Duration;

use async_nats::jetstream::kv;
use bytes::Bytes;
use trogon_nats::jetstream::{
    JetStreamKeyValueCreateWithTtl, JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueUpdate,
};
use trogon_nats::lease::{LeaseKey, ReleaseLease, RenewLease, TryAcquireLease};

/// Per-session NATS KV lease implementing `trogon_nats::lease` traits.
#[derive(Clone)]
pub struct SessionKvLease<S> {
    store: S,
    key: LeaseKey,
    ttl: Duration,
}

impl<S> SessionKvLease<S> {
    pub fn new(store: S, key: LeaseKey, ttl: Duration) -> Self {
        Self { store, key, ttl }
    }
}

impl<S> TryAcquireLease for SessionKvLease<S>
where
    S: JetStreamKeyValueCreateWithTtl + Clone + Send + Sync + 'static,
{
    type Error = kv::CreateError;

    async fn try_acquire(&self, value: Bytes) -> Result<u64, Self::Error> {
        self.store.create_with_ttl(self.key.as_str(), value, self.ttl).await
    }
}

impl<S> RenewLease for SessionKvLease<S>
where
    S: JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    type Error = kv::UpdateError;

    async fn renew(&self, value: Bytes, revision: u64) -> Result<u64, Self::Error> {
        self.store.update(self.key.as_str(), value, revision).await
    }
}

impl<S> ReleaseLease for SessionKvLease<S>
where
    S: JetStreamKeyValueDeleteExpectRevision + Clone + Send + Sync + 'static,
{
    type Error = kv::DeleteError;

    async fn release(&self, revision: u64) -> Result<(), Self::Error> {
        self.store
            .delete_expect_revision(self.key.as_str(), Some(revision))
            .await
    }
}
