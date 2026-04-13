use std::collections::BTreeMap;

use async_nats::jetstream::{self, kv};
use trogon_eventsourcing::{load_snapshot, maybe_advance_checkpoint, persist_snapshot_change};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{error::CronError, events::JobEventData, nats::apply_event_to_snapshot_map};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

pub(super) async fn run<J>(js: &J, job_id: &str, events: &[JobEventData]) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    if events.is_empty() {
        return Ok(());
    }

    let bucket = snapshot_bucket::run(js).await?;
    let mut snapshots = BTreeMap::new();
    if let Some(snapshot) = load_snapshot(&bucket, SNAPSHOT_STORE_CONFIG, job_id)
        .await
        .map_err(CronError::from)?
    {
        snapshots.insert(job_id.to_string(), snapshot);
    }

    let stream = append_events::stream_subject_state(js, job_id).await?;
    let final_version = stream.write_state.current_version().ok_or_else(|| {
        CronError::event_source(
            "stream snapshot projection requires an event version",
            std::io::Error::other(format!("job '{job_id}'")),
        )
    })?;
    let start_version = final_version
        .checked_sub(events.len() as u64 - 1)
        .ok_or_else(|| {
            CronError::event_source(
                "stream snapshot projection requires a valid batch version range",
                std::io::Error::other(format!("job '{job_id}'")),
            )
        })?;

    for (index, event) in events.iter().enumerate() {
        let change =
            apply_event_to_snapshot_map(&mut snapshots, &event.data, start_version + index as u64)?;
        persist_snapshot_change(&bucket, SNAPSHOT_STORE_CONFIG, change)
            .await
            .map_err(CronError::from)?;
    }
    maybe_advance_checkpoint(&bucket, SNAPSHOT_STORE_CONFIG, final_version)
        .await
        .map_err(CronError::from)
}
