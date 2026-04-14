#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobEnabledState, JobId, JobSpec, JobWriteCondition,
    error::CronError,
    events::{JobEvent, JobEventData},
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct ChangeJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
    pub write_condition: JobWriteCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeJobStateState {
    Missing,
    Present { current: JobEnabledState },
}

impl StreamCommand for ChangeJobStateCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl ChangeJobStateCommand {
    pub fn state_from_snapshot(
        &self,
        snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> Result<ChangeJobStateState, CronError> {
        match snapshot {
            None => Ok(ChangeJobStateState::Missing),
            Some(snapshot) if snapshot.payload.id == self.stream_id().as_str() => {
                Ok(ChangeJobStateState::Present {
                    current: snapshot.payload.state,
                })
            }
            Some(snapshot) => Err(CronError::event_source(
                "failed to decode current job snapshot into change-job-state state",
                std::io::Error::other(format!(
                    "expected '{}' but snapshot carried '{}'",
                    self.stream_id(),
                    snapshot.payload.id
                )),
            )),
        }
    }
}

impl Decide<ChangeJobStateState, JobEvent> for ChangeJobStateCommand {
    type Error = JobDecisionError;

    fn decide(
        state: &ChangeJobStateState,
        command: &Self,
    ) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            ChangeJobStateState::Missing => Err(JobDecisionError::MissingJobForStateChange {
                id: command.stream_id().clone(),
            }),
            ChangeJobStateState::Present { current } if *current == command.state => {
                Err(JobDecisionError::StateAlreadySet {
                    id: command.stream_id().clone(),
                    state: command.state,
                })
            }
            ChangeJobStateState::Present { .. } => Ok(Decision::Event(NonEmpty::one(
                JobEvent::job_state_changed(command.stream_id().to_string(), command.state),
            ))),
        }
    }
}

#[cfg(not(coverage))]
pub async fn run<J>(js: &J, command: ChangeJobStateCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let ChangeJobStateCommand {
        id,
        state,
        write_condition,
    } = command;

    let bucket = snapshot_bucket::run(js).await?;
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, id.as_str())
        .await
        .map_err(CronError::from)?;
    let command = ChangeJobStateCommand {
        id: id.clone(),
        state,
        write_condition,
    };
    let current_state = command.state_from_snapshot(current_snapshot.as_ref())?;
    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job state change from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::MissingJobForStateChange { .. }) => {
            return Err(CronError::JobNotFound { id: id.to_string() });
        }
        Err(JobDecisionError::StateAlreadySet { state, .. }) => {
            return Err(CronError::JobStateAlreadySet {
                id: id.to_string(),
                state,
            });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to decide job state change from current change-job-state state",
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
pub async fn run<J>(_js: &J, _command: ChangeJobStateCommand) -> Result<(), CronError>
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
