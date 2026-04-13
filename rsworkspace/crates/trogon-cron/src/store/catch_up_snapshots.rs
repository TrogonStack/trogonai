#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv};
use trogon_eventsourcing::{
    load_snapshot_map, persist_snapshot_change, read_checkpoint, write_checkpoint,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    error::CronError,
    nats::{
        apply_event_to_snapshot_map, decode_recorded_job_event, job_id_from_event_subject,
        read_raw_event_message,
    },
};

use super::{SNAPSHOT_STORE_CONFIG, events_stream, snapshot_bucket};

#[cfg(not(coverage))]
pub(super) async fn run<J>(js: &J) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream = events_stream::run(js).await?;
    let info = stream.get_info().await.map_err(|source| {
        CronError::event_source(
            "failed to query events stream info for snapshot catch-up",
            source,
        )
    })?;
    if info.state.messages == 0 {
        return Ok(());
    }

    let bucket = snapshot_bucket::run(js).await?;
    let checkpoint = read_checkpoint(&bucket, SNAPSHOT_STORE_CONFIG)
        .await
        .map_err(CronError::from)?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    let mut snapshots = load_snapshot_map(&bucket, SNAPSHOT_STORE_CONFIG)
        .await
        .map_err(CronError::from)?;
    let start = checkpoint.max(info.state.first_sequence.saturating_sub(1)) + 1;

    for sequence in start..=info.state.last_sequence {
        let Some(message) = read_raw_event_message(
            &stream,
            sequence,
            "failed to read job event during snapshot catch-up",
        )
        .await?
        else {
            continue;
        };
        let event = decode_recorded_job_event(message)?;
        let stream_id = job_id_from_event_subject(&event.stream_id)?;
        let change =
            apply_event_to_snapshot_map(&mut snapshots, &stream_id, &event.data, sequence)?;
        persist_snapshot_change(&bucket, SNAPSHOT_STORE_CONFIG, change)
            .await
            .map_err(CronError::from)?;
        write_checkpoint(&bucket, SNAPSHOT_STORE_CONFIG, sequence)
            .await
            .map_err(CronError::from)?;
    }

    Ok(())
}

#[cfg(coverage)]
pub(super) async fn run<J>(_js: &J) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    Ok(())
}
