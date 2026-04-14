use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobSpec, JobWriteCondition,
    commands::catch_up_command_state,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecisionError {
    JobNotFound { id: JobId },
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
        current_version: Option<u64>,
    ) -> JobWriteCondition {
        self.write_condition.unwrap_or_else(|| {
            current_version
                .map(JobWriteCondition::MustBeAtVersion)
                .unwrap_or(JobWriteCondition::MustNotExist)
        })
    }

    fn apply_event(_state: RemoveJobState, event: JobEvent) -> Result<RemoveJobState, CronError> {
        match event {
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => {
                Ok(RemoveJobState::Present)
            }
            JobEvent::JobRemoved { .. } => Ok(RemoveJobState::Missing),
        }
    }
}

impl std::fmt::Display for RemoveJobDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for removal '{id}'"),
        }
    }
}

impl std::error::Error for RemoveJobDecisionError {}

impl StreamCommand for RemoveJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<RemoveJobState, JobEvent> for RemoveJobCommand {
    type Error = RemoveJobDecisionError;

    fn decide(state: &RemoveJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RemoveJobState::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            RemoveJobState::Present => Ok(Decision::Event(NonEmpty::one(JobEvent::JobRemoved {
                id: command.stream_id().to_string(),
            }))),
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
    let (current_state, current_version) = catch_up_command_state(
        js,
        command.stream_id(),
        current_snapshot.as_ref(),
        current_state,
        RemoveJobCommand::apply_event,
    )
    .await?;
    let write_condition = command.resolved_write_condition(current_version);
    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(RemoveJobDecisionError::JobNotFound { .. }) => {
            return Err(CronError::JobNotFound { id });
        }
    };

    let events = events.try_map(JobEventData::new)?;
    append_events(js, command.stream_id().as_str(), write_condition, events).await
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

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{Decision, NonEmpty, decide};

    use super::*;

    #[test]
    fn decides_removal_from_present_state() {
        let state = RemoveJobState::Present;
        let command = RemoveJobCommand::with_write_condition(
            JobId::parse("backup").unwrap(),
            JobWriteCondition::MustBeAtVersion(1),
        );

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobRemoved {
                id: "backup".to_string(),
            }))
        );
    }

    #[test]
    fn rejects_removing_missing_job() {
        let state = RemoveJobState::Missing;
        let command = RemoveJobCommand::with_write_condition(
            JobId::parse("backup").unwrap(),
            JobWriteCondition::MustBeAtVersion(1),
        );

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RemoveJobDecisionError::JobNotFound { .. }
        ));
    }
}
