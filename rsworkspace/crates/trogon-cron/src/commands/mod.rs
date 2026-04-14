use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{NonEmpty, Snapshot, StreamCommand, load_snapshot, read_stream_range};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobSpec, JobWriteCondition,
    error::CronError,
    events::{JobEvent, JobEventData},
    store::{append_events, open_events_stream, open_snapshot_bucket, stream_subject_state},
};

mod add;
mod get;
mod list;
mod remove;
mod set_state;

pub use add::{
    RegisterJobCommand, RegisterJobDecisionError, RegisterJobState, run as register_job,
};
pub use get::{GetJobCommand, run as get_job};
pub use list::{ListJobsCommand, run as list_jobs};
pub use remove::{RemoveJobCommand, RemoveJobDecisionError, RemoveJobState, run as remove_job};
pub use set_state::{
    ChangeJobStateCommand, ChangeJobStateDecisionError, ChangeJobStateState,
    run as change_job_state,
};

pub(crate) trait Evolve: StreamCommand<StreamId = JobId> {
    type State;

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, CronError>;
}

#[doc(hidden)]
pub trait CommandRuntime: Send + Sync {
    fn load_job_snapshot(
        &self,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<JobSpec>>, CronError>> + Send;

    fn current_job_stream_version(
        &self,
        stream_id: &JobId,
    ) -> impl std::future::Future<Output = Result<Option<u64>, CronError>> + Send;

    fn read_job_stream_events(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
        to_sequence: u64,
    ) -> impl std::future::Future<Output = Result<Vec<JobEvent>, CronError>> + Send;

    fn append_job_events(
        &self,
        stream_id: &JobId,
        write_condition: JobWriteCondition,
        events: NonEmpty<JobEventData>,
    ) -> impl std::future::Future<Output = Result<(), CronError>> + Send;
}

impl<J> CommandRuntime for J
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        > + Send
        + Sync,
{
    async fn load_job_snapshot(
        &self,
        stream_id: &JobId,
    ) -> Result<Option<Snapshot<JobSpec>>, CronError> {
        let bucket = open_snapshot_bucket(self).await?;
        load_snapshot::<JobSpec>(
            &bucket,
            crate::store::SNAPSHOT_STORE_CONFIG,
            stream_id.as_str(),
        )
        .await
        .map_err(CronError::from)
    }

    async fn current_job_stream_version(
        &self,
        stream_id: &JobId,
    ) -> Result<Option<u64>, CronError> {
        Ok(stream_subject_state(self, stream_id.as_str())
            .await?
            .write_state
            .current_version())
    }

    async fn read_job_stream_events(
        &self,
        stream_id: &JobId,
        from_sequence: u64,
        to_sequence: u64,
    ) -> Result<Vec<JobEvent>, CronError> {
        let stream = open_events_stream(self).await?;
        let recorded_events = read_stream_range(&stream, from_sequence, to_sequence)
            .await
            .map_err(|source| {
                CronError::event_source(
                    "failed to read job stream while catching up command state",
                    source,
                )
            })?;

        let mut events = Vec::new();
        for recorded in recorded_events {
            if recorded.stream_id() != stream_id.as_str() {
                continue;
            }
            events.push(recorded.decode_data::<JobEvent>().map_err(|source| {
                CronError::event_source(
                    "failed to decode job event while catching up command state",
                    source,
                )
            })?);
        }
        Ok(events)
    }

    async fn append_job_events(
        &self,
        stream_id: &JobId,
        write_condition: JobWriteCondition,
        events: NonEmpty<JobEventData>,
    ) -> Result<(), CronError> {
        append_events(self, stream_id.as_str(), write_condition, events).await
    }
}

pub(crate) async fn catch_up_command_state<R, C>(
    runtime: &R,
    command: &C,
    snapshot: Option<&Snapshot<JobSpec>>,
    mut state: C::State,
) -> Result<(C::State, Option<u64>), CronError>
where
    R: CommandRuntime,
    C: Evolve,
{
    let stream_id = command.stream_id();
    let current_version = runtime.current_job_stream_version(stream_id).await?;
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

    for event in runtime
        .read_job_stream_events(stream_id, start_sequence, current_version)
        .await?
    {
        state = C::evolve(state, event)?;
    }

    Ok((state, Some(current_version)))
}
