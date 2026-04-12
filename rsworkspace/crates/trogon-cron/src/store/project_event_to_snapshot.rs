use std::collections::BTreeMap;

use async_nats::jetstream::{self, kv};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    error::CronError,
    events::JobEventData,
    nats::{
        apply_event_to_snapshot_map, maybe_advance_snapshot_checkpoint, persist_snapshot_change,
    },
};

use super::{append_events, load_snapshot, snapshot_bucket};

pub(super) async fn run<J>(js: &J, job_id: &str, event: &JobEventData) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let bucket = snapshot_bucket::run(js).await?;
    let mut snapshots = BTreeMap::new();
    if let Some(snapshot) = load_snapshot::run(js, job_id).await? {
        snapshots.insert(job_id.to_string(), snapshot);
    }

    let aggregate = append_events::aggregate_subject_state(js, job_id).await?;
    let final_version = aggregate.write_state.current_version().ok_or_else(|| {
        CronError::event_source(
            "aggregate snapshot projection requires an event version",
            std::io::Error::other(format!("job '{job_id}'")),
        )
    })?;

    let change = apply_event_to_snapshot_map(&mut snapshots, &event.data, final_version)?;
    persist_snapshot_change(&bucket, change).await?;
    maybe_advance_snapshot_checkpoint(&bucket, final_version).await
}
