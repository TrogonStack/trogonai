//! The read model's KV storage contract: the bucket it lives in, the key scheme,
//! and the catch-up checkpoint key.
//!
//! This belongs to the projection because the projection owns the storage layout
//! — it decides how schedules are keyed and stored. The query side reads the same
//! bucket, so it depends on this contract; the shared NATS plumbing for the event
//! stream and other buckets stays in [`crate::kv`].

#![cfg_attr(coverage, allow(dead_code))]

use async_nats::jetstream::kv;
use trogon_nats::jetstream::{JetStreamCreateKeyValue, JetStreamGetKeyValue};

use crate::error::SchedulerError;
use crate::processor::execution::reconciliation::{ScheduleKey, StreamRoutingId};

/// KV bucket holding the schedules read model.
pub const SCHEDULES_BUCKET: &str = "scheduler_schedules";

/// Key of the catch-up checkpoint entry within [`SCHEDULES_BUCKET`].
///
/// Versioned: the read model keys entries by a derived token (see
/// [`read_model_key`]) rather than the raw schedule id. The version forces a
/// one-time full rebuild on upgrade so the bucket is re-keyed under the new
/// scheme; the catch-up GC sweep then removes any entry written under the old
/// (raw-id) scheme.
pub const SCHEDULES_CHECKPOINT_KEY: &str = "_query.schedules.read_model.v2.last_event_sequence";

/// Derives the KV key for a schedule's read-model entry.
///
/// A schedule id may be any string the command domain allows — dots, slashes,
/// `:`, `@`, non-ASCII, up to 256 chars — which is not always a valid NATS KV
/// key. Keying by a derived 32-hex token (the same scheme the execution worker
/// uses for its checkpoint) makes every schedule both storable and addressable,
/// regardless of the characters in its id.
pub(crate) fn read_model_key(schedule_id: &str) -> String {
    ScheduleKey::for_stream(&StreamRoutingId::from(schedule_id)).simple()
}

#[cfg(not(coverage))]
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

#[cfg(coverage)]
pub(crate) async fn get_or_create_schedules_bucket<J>(_js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamCreateKeyValue<Store = kv::Store> + JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(SchedulerError::kv_source(
        "coverage stub does not provision schedules buckets",
        std::io::Error::other(SCHEDULES_BUCKET),
    ))
}

#[cfg(not(coverage))]
pub(crate) async fn open_schedules_bucket<J>(js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(SCHEDULES_BUCKET)
        .await
        .map_err(|source| SchedulerError::kv_source("failed to open schedules bucket", source))
}

#[cfg(coverage)]
pub(crate) async fn open_schedules_bucket<J>(_js: &J) -> Result<kv::Store, SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(SchedulerError::kv_source(
        "coverage stub does not open the schedules bucket",
        std::io::Error::other(SCHEDULES_BUCKET),
    ))
}
