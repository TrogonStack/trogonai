#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobEnabledState, JobId, JobSpec, JobWriteCondition,
    commands::catch_up_command_state,
    error::CronError,
    events::{JobEvent, JobEventData},
    store::{SNAPSHOT_STORE_CONFIG, append_events, open_snapshot_bucket},
};

#[derive(Debug, Clone)]
pub struct ChangeJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
    write_condition: Option<JobWriteCondition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeJobStateState {
    Missing,
    Present { current: JobEnabledState },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeJobStateDecisionError {
    JobNotFound { id: JobId },
    StateAlreadySet { id: JobId, state: JobEnabledState },
}

impl ChangeJobStateCommand {
    pub const fn new(id: JobId, state: JobEnabledState) -> Self {
        Self {
            id,
            state,
            write_condition: None,
        }
    }

    pub const fn with_write_condition(
        id: JobId,
        state: JobEnabledState,
        write_condition: JobWriteCondition,
    ) -> Self {
        Self {
            id,
            state,
            write_condition: Some(write_condition),
        }
    }

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

    fn apply_event(
        _state: ChangeJobStateState,
        event: JobEvent,
    ) -> Result<ChangeJobStateState, CronError> {
        match event {
            JobEvent::JobRegistered { id, spec } => Ok(ChangeJobStateState::Present {
                current: spec.into_job_spec(id).state,
            }),
            JobEvent::JobStateChanged { state, .. } => {
                Ok(ChangeJobStateState::Present { current: state })
            }
            JobEvent::JobRemoved { .. } => Ok(ChangeJobStateState::Missing),
        }
    }
}

impl std::fmt::Display for ChangeJobStateDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for state change '{id}'"),
            Self::StateAlreadySet { id, state } => {
                write!(f, "job '{id}' is already {}", state.as_str())
            }
        }
    }
}

impl std::error::Error for ChangeJobStateDecisionError {}

impl StreamCommand for ChangeJobStateCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<ChangeJobStateState, JobEvent> for ChangeJobStateCommand {
    type Error = ChangeJobStateDecisionError;

    fn decide(
        state: &ChangeJobStateState,
        command: &Self,
    ) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            ChangeJobStateState::Missing => Err(ChangeJobStateDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            ChangeJobStateState::Present { current } if *current == command.state => {
                Err(ChangeJobStateDecisionError::StateAlreadySet {
                    id: command.stream_id().clone(),
                    state: command.state,
                })
            }
            ChangeJobStateState::Present { .. } => {
                Ok(Decision::Event(NonEmpty::one(JobEvent::JobStateChanged {
                    id: command.stream_id().to_string(),
                    state: command.state,
                })))
            }
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
        ChangeJobStateCommand::apply_event,
    )
    .await?;
    let write_condition = command.resolved_write_condition(current_version);
    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job state change from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(ChangeJobStateDecisionError::JobNotFound { .. }) => {
            return Err(CronError::JobNotFound { id });
        }
        Err(ChangeJobStateDecisionError::StateAlreadySet { state, .. }) => {
            return Err(CronError::JobStateAlreadySet { id, state });
        }
    };

    let events = events.try_map(JobEventData::new)?;
    append_events(js, command.stream_id().as_str(), write_condition, events).await
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

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{Decision, NonEmpty, decide};

    use super::*;

    #[test]
    fn decides_state_change_from_present_state() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command = ChangeJobStateCommand::with_write_condition(
            JobId::parse("backup").unwrap(),
            JobEnabledState::Disabled,
            JobWriteCondition::MustBeAtVersion(1),
        );

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobStateChanged {
                id: "backup".to_string(),
                state: JobEnabledState::Disabled,
            }))
        );
    }

    #[test]
    fn rejects_noop_state_changes() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command = ChangeJobStateCommand::with_write_condition(
            JobId::parse("backup").unwrap(),
            JobEnabledState::Enabled,
            JobWriteCondition::MustBeAtVersion(1),
        );

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::StateAlreadySet { .. }
        ));
    }

    #[test]
    fn rejects_state_changes_for_missing_jobs() {
        let state = ChangeJobStateState::Missing;
        let command = ChangeJobStateCommand::with_write_condition(
            JobId::parse("backup").unwrap(),
            JobEnabledState::Enabled,
            JobWriteCondition::MustBeAtVersion(1),
        );

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::JobNotFound { .. }
        ));
    }
}
