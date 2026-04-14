use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobId, JobSpec, JobWriteCondition,
    error::CronError,
    events::{JobEvent, JobEventData},
    store::{SNAPSHOT_STORE_CONFIG, append_events, open_snapshot_bucket},
};

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
    write_condition: Option<JobWriteCondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveJobState {
    Missing,
    Present,
}

impl RemoveJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self {
            id,
            write_condition: None,
        }
    }

    pub const fn with_write_condition(id: JobId, write_condition: JobWriteCondition) -> Self {
        Self {
            id,
            write_condition: Some(write_condition),
        }
    }

    pub fn state_from_snapshot(
        &self,
        snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> Result<RemoveJobState, CronError> {
        match snapshot {
            None => Ok(RemoveJobState::Missing),
            Some(snapshot) if snapshot.payload.id == self.stream_id().as_str() => {
                Ok(RemoveJobState::Present)
            }
            Some(snapshot) => Err(CronError::event_source(
                "failed to decode current job snapshot into remove-job state",
                std::io::Error::other(format!(
                    "expected '{}' but snapshot carried '{}'",
                    self.stream_id(),
                    snapshot.payload.id
                )),
            )),
        }
    }

    pub(crate) fn resolved_write_condition(
        &self,
        current_snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> JobWriteCondition {
        self.write_condition.unwrap_or_else(|| {
            current_snapshot
                .map(|job| JobWriteCondition::MustBeAtVersion(job.version))
                .unwrap_or(JobWriteCondition::MustNotExist)
        })
    }
}

impl StreamCommand for RemoveJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<RemoveJobState, JobEvent> for RemoveJobCommand {
    type Error = JobDecisionError;

    fn decide(state: &RemoveJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RemoveJobState::Missing => Err(JobDecisionError::MissingJobForRemoval {
                id: command.stream_id().clone(),
            }),
            RemoveJobState::Present => Ok(Decision::Event(NonEmpty::one(JobEvent::job_removed(
                command.stream_id().to_string(),
            )))),
        }
    }
}

#[cfg(not(coverage))]
pub async fn run<J>(js: &J, command: RemoveJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let id = command.stream_id().to_string();
    let bucket = open_snapshot_bucket(js).await?;
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, &id)
        .await
        .map_err(CronError::from)?;
    let current_state = command.state_from_snapshot(current_snapshot.as_ref())?;
    let write_condition = command.resolved_write_condition(current_snapshot.as_ref());
    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::MissingJobForRemoval { .. }) => {
            return Err(CronError::JobNotFound { id });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current remove-job state",
                error,
            ));
        }
    };

    append_events(
        js,
        command.stream_id().as_str(),
        write_condition,
        events.map(JobEventData::new),
    )
    .await
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J, _command: RemoveJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    Ok(())
}
