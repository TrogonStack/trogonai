use async_nats::jetstream::{self, kv};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use crate::{
    error::CronError,
    nats::{
        apply_event_to_snapshot_map, decode_recorded_job_event, load_snapshot_map,
        persist_snapshot_change, read_raw_event_message, read_snapshot_checkpoint,
        write_snapshot_checkpoint,
    },
};

use super::{events_stream, snapshot_bucket};

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
    let checkpoint = read_snapshot_checkpoint(&bucket).await?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    let mut snapshots = load_snapshot_map(&bucket).await?;
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
        let change = apply_event_to_snapshot_map(&mut snapshots, &event.data, sequence)?;
        persist_snapshot_change(&bucket, change).await?;
        write_snapshot_checkpoint(&bucket, sequence).await?;
    }

    Ok(())
}
