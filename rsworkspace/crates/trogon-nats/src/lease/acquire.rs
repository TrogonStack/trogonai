use async_nats::jetstream::kv;
use bytes::Bytes;

use super::{NatsKvLease, TryAcquireLease};

impl TryAcquireLease for NatsKvLease {
    type Error = kv::CreateError;

    async fn try_acquire(&self, value: Bytes) -> Result<u64, Self::Error> {
        match self.ttl {
            Some(ttl) => {
                self.store
                    .create_with_ttl(self.key.as_str(), value, ttl)
                    .await
            }
            None => self.store.create(self.key.as_str(), value).await,
        }
    }
}
