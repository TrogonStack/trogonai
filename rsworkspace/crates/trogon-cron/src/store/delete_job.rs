#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobId, JobSpec, JobWriteCondition,
    error::CronError,
    events::{JobEvent, JobEventData},
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct DeleteJobCommand {
    pub id: JobId,
    pub write_condition: JobWriteCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteJobState {
    Missing,
    Present,
}

impl StreamCommand for DeleteJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl DeleteJobCommand {
    pub fn state_from_snapshot(
        &self,
        snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> Result<DeleteJobState, CronError> {
        match snapshot {
            None => Ok(DeleteJobState::Missing),
            Some(snapshot) if snapshot.payload.id == self.stream_id().as_str() => {
                Ok(DeleteJobState::Present)
            }
            Some(snapshot) => Err(CronError::event_source(
                "failed to decode current job snapshot into delete-job state",
                std::io::Error::other(format!(
                    "expected '{}' but snapshot carried '{}'",
                    self.stream_id(),
                    snapshot.payload.id
                )),
            )),
        }
    }
}

impl Decide<DeleteJobState, JobEvent> for DeleteJobCommand {
    type Error = JobDecisionError;

    fn decide(state: &DeleteJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            DeleteJobState::Missing => Err(JobDecisionError::MissingJobForRemoval {
                id: command.stream_id().clone(),
            }),
            DeleteJobState::Present => Ok(Decision::Event(NonEmpty::one(JobEvent::job_removed(
                command.stream_id().to_string(),
            )))),
        }
    }
}

#[cfg(not(coverage))]
pub async fn run<J>(js: &J, command: DeleteJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let DeleteJobCommand {
        id,
        write_condition,
    } = command;

    let bucket = snapshot_bucket::run(js).await?;
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, id.as_str())
        .await
        .map_err(CronError::from)?;
    let command = DeleteJobCommand {
        id: id.clone(),
        write_condition,
    };
    let current_state = command.state_from_snapshot(current_snapshot.as_ref())?;
    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::MissingJobForRemoval { .. }) => {
            return Err(CronError::JobNotFound { id: id.to_string() });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current delete-job state",
                error,
            ));
        }
    };

    append_events::run(
        js,
        id.as_str(),
        command.write_condition,
        events.map(JobEventData::new),
    )
    .await
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J, _command: DeleteJobCommand) -> Result<(), CronError>
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
