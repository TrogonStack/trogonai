use async_nats::jetstream;
use trogon_eventsourcing::{Snapshot, read_stream_range};
use trogon_nats::jetstream::JetStreamGetStream;

use crate::{
    JobId, JobSpec,
    error::CronError,
    events::JobEvent,
    store::{open_events_stream, stream_subject_state},
};

mod add;
mod get;
mod list;
mod remove;
mod set_state;

pub use add::{
    ReadRegisterJobCommandError, RegisterJobCommand, RegisterJobDecisionError, RegisterJobState,
    read_from_stdin, run as register_job,
};
pub use get::{GetJobCommand, run as get_job};
pub use list::{ListJobsCommand, run as list_jobs};
pub use remove::{RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, run as remove_job};
pub use set_state::{
    ChangeJobStateCommand, ChangeJobStateDecisionError, ChangeJobStateState,
    run as change_job_state,
};

async fn catch_up_command_state<J, S, F>(
    js: &J,
    stream_id: &JobId,
    snapshot: Option<&Snapshot<JobSpec>>,
    mut state: S,
    mut apply: F,
) -> Result<(S, Option<u64>), CronError>
where
    J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
    F: FnMut(S, JobEvent) -> Result<S, CronError>,
{
    let stream_state = stream_subject_state(js, stream_id.as_str()).await?;
    let current_version = stream_state.write_state.current_version();
    let snapshot_version = snapshot.map(|snapshot| snapshot.version);

    match (snapshot_version, current_version) {
        (Some(snapshot_version), Some(current_version)) if snapshot_version > current_version => {
            return Err(CronError::event_source(
                "loaded job snapshot is ahead of the stream state",
                std::io::Error::other(format!(
                    "job '{}' snapshot version {} > stream version {}",
                    stream_id, snapshot_version, current_version
                )),
            ));
        }
        (Some(snapshot_version), None) => {
            return Err(CronError::event_source(
                "loaded job snapshot exists without any stream history",
                std::io::Error::other(format!(
                    "job '{}' snapshot version {}",
                    stream_id, snapshot_version
                )),
            ));
        }
        _ => {}
    }

    let Some(current_version) = current_version else {
        return Ok((state, None));
    };
    let start_sequence = snapshot_version
        .map(|version| version.saturating_add(1))
        .unwrap_or(1);
    if start_sequence > current_version {
        return Ok((state, Some(current_version)));
    }

    let stream = open_events_stream(js).await?;
    let recorded_events = read_stream_range(&stream, start_sequence, current_version)
        .await
        .map_err(|source| {
            CronError::event_source(
                "failed to read job stream while catching up command state",
                source,
            )
        })?;

    for recorded in recorded_events {
        if recorded.stream_id() != stream_id.as_str() {
            continue;
        }
        let event = recorded.decode_data::<JobEvent>().map_err(|source| {
            CronError::event_source(
                "failed to decode job event while catching up command state",
                source,
            )
        })?;
        state = apply(state, event)?;
    }

    Ok((state, Some(current_version)))
}
