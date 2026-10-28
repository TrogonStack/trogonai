//! The read model's KV storage contract: the bucket it lives in, the key scheme,
//! and the catch-up checkpoint key.
//!
//! This belongs to the projection because the projection owns the storage layout
//! — it decides how schedules are keyed and stored. The query side reads the same
//! bucket, so it depends on this contract; the shared NATS plumbing for the event
//! stream and other buckets stays in [`crate::kv`].

use async_nats::jetstream::kv;
use trogon_nats::jetstream::{JetStreamCreateKeyValue, JetStreamGetKeyValue};

use crate::constants::SCHEDULES_BUCKET;
use crate::error::SchedulerError;
pub(crate) async fn get_or_create_schedules_bucket<J>(js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamCreateKeyValue<Store = kv::Store> + JetStreamGetKeyValue<Store = kv::Store>,
{
    // Provision the bucket on first use (a fresh JetStream, or a first deploy),
    // then fall back to opening it if a peer created it first. Mirrors the shared
    // lease-bucket provisioning in `trogon_nats`.
    match js
        .create_key_value(kv::Config {
            bucket: SCHEDULES_BUCKET.to_string(),
            history: 5,
            ..Default::default()
        })
        .await
    {
        Ok(store) => Ok(store),
        Err(source) if trogon_nats::jetstream::is_create_key_value_already_exists(&source) => {
            open_schedules_bucket(js).await
        }
        Err(source) => Err(SchedulerError::kv_source("failed to create schedules bucket", source)),
    }
}

pub(crate) async fn open_schedules_bucket<J>(js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(SCHEDULES_BUCKET)
        .await
        .map_err(|source| SchedulerError::kv_source("failed to open schedules bucket", source))
}
