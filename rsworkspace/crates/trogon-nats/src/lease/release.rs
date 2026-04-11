use async_nats::jetstream::kv;

use super::{NatsKvLease, ReleaseLease};

impl ReleaseLease for NatsKvLease {
    type Error = kv::DeleteError;

    async fn release(&self, revision: u64) -> Result<(), Self::Error> {
        self.store
            .delete_expect_revision(self.key.as_str(), Some(revision))
            .await
    }
}
