use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandFailure, CommandInfraError, CommandOutcome,
    CommandState, Decide, Decision, EventStore, NonEmpty, OccPolicy, SnapshotState, SnapshotStore,
    SnapshotStoreConfig, Snapshots, StreamCommand,
};

use crate::{
    JobEnabledState, JobId, JobIdError,
    error::CronError,
    events::{JobEvent, JobEventCodec},
};

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.change_job_state.v1.", None);

#[derive(Debug, Clone)]
pub struct ChangeJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeJobStateState {
    Missing,
    Present { current: JobEnabledState },
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeJobStateDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    StateAlreadySet { id: JobId, state: JobEnabledState },
}

impl ChangeJobStateCommand {
    pub const fn new(id: JobId, state: JobEnabledState) -> Self {
        Self { id, state }
    }
}

impl std::fmt::Display for ChangeJobStateDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for state change '{id}'"),
            Self::JobDeleted { id } => {
                write!(f, "job '{id}' was deleted and cannot change state")
            }
            Self::StateAlreadySet { id, state } => {
                write!(f, "job '{id}' is already {}", state.as_str())
            }
        }
    }
}

impl std::error::Error for ChangeJobStateDecisionError {}

#[derive(Debug)]
pub enum ChangeJobStateError {
    Decision(ChangeJobStateDecisionError),
    InvalidRegistrationEventId { id: String, source: JobIdError },
}

impl std::fmt::Display for ChangeJobStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decision(error) => write!(f, "{error}"),
            Self::InvalidRegistrationEventId { id, .. } => {
                write!(
                    f,
                    "invalid job id '{id}' in registration event while changing job state"
                )
            }
        }
    }
}

impl std::error::Error for ChangeJobStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decision(error) => Some(error),
            Self::InvalidRegistrationEventId { source, .. } => Some(source),
        }
    }
}

impl From<ChangeJobStateDecisionError> for ChangeJobStateError {
    fn from(value: ChangeJobStateDecisionError) -> Self {
        Self::Decision(value)
    }
}

pub type ChangeJobStateResult = Result<
    CommandOutcome<JobEvent>,
    CommandFailure<ChangeJobStateError, CommandInfraError<CronError>>,
>;

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
            ChangeJobStateState::Deleted => Err(ChangeJobStateDecisionError::JobDeleted {
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

impl CommandState for ChangeJobStateCommand {
    type State = ChangeJobStateState;
    type Event = JobEvent;
    type DomainError = ChangeJobStateError;

    fn initial_state() -> Self::State {
        ChangeJobStateState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { id, spec } => {
                if matches!(state, ChangeJobStateState::Deleted) {
                    return Ok(ChangeJobStateState::Deleted);
                }
                let job_id = JobId::parse(&id).map_err(|source| {
                    ChangeJobStateError::InvalidRegistrationEventId {
                        id: id.clone(),
                        source,
                    }
                })?;
                Ok(ChangeJobStateState::Present {
                    current: spec.into_job_spec(job_id).state,
                })
            }
            JobEvent::JobStateChanged {
                state: current_state,
                ..
            } => match state {
                ChangeJobStateState::Deleted => Ok(ChangeJobStateState::Deleted),
                ChangeJobStateState::Missing | ChangeJobStateState::Present { .. } => {
                    Ok(ChangeJobStateState::Present {
                        current: current_state,
                    })
                }
            },
            JobEvent::JobRemoved { .. } => Ok(ChangeJobStateState::Deleted),
        }
    }
}

impl SnapshotState for ChangeJobStateCommand {
    type Snapshot = ChangeJobStateState;

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
        Some(*state)
    }
}

pub async fn run<E, S>(
    event_store: &E,
    snapshot_store: &S,
    command: ChangeJobStateCommand,
    occ: Option<OccPolicy>,
) -> ChangeJobStateResult
where
    E: EventStore<JobId, Error = CronError>,
    S: SnapshotStore<ChangeJobStateState, JobId, Error = CronError>,
{
    CommandExecution::new(event_store, &command)
        .codec(JobEventCodec)
        .occ(occ)
        .snapshots(Snapshots::new(
            snapshot_store,
            SNAPSHOT_STORE_CONFIG,
            AlwaysSnapshot,
        ))
        .execute()
        .await
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{
        Decision, NonEmpty, Snapshot, decide,
        testing::{TestCase, Timeline, decider, expect_error},
    };

    use super::*;
    use crate::{
        DeliverySpec, GetJobCommand, JobSpec, RegisterJobCommand, ScheduleSpec,
        mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: std::collections::BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn decides_state_change_from_present_state() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Disabled);

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
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Enabled);

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::StateAlreadySet { .. }
        ));
    }

    #[test]
    fn rejects_state_changes_for_missing_jobs() {
        let state = ChangeJobStateState::Missing;
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Enabled);

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_state_changes_for_deleted_jobs() {
        let state = ChangeJobStateState::Deleted;
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Enabled);

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::JobDeleted { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_change_state_decider() {
        TestCase::new(decider::<ChangeJobStateCommand>())
            .given([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: crate::RegisteredJobSpec::from(job("backup")),
            }])
            .when(ChangeJobStateCommand::new(
                JobId::parse("backup").unwrap(),
                JobEnabledState::Disabled,
            ))
            .then([JobEvent::JobStateChanged {
                id: "backup".to_string(),
                state: JobEnabledState::Disabled,
            }]);
    }

    #[test]
    fn given_when_then_supports_change_state_failures() {
        TestCase::new(decider::<ChangeJobStateCommand>())
            .given([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: crate::RegisteredJobSpec::from(job("backup")),
            }])
            .when(ChangeJobStateCommand::new(
                JobId::parse("backup").unwrap(),
                JobEnabledState::Enabled,
            ))
            .then(expect_error(ChangeJobStateDecisionError::StateAlreadySet {
                id: JobId::parse("backup").unwrap(),
                state: JobEnabledState::Enabled,
            }));
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::new(decider::<RegisterJobCommand>())
            .given([])
            .when(RegisterJobCommand::new(job("backup")).unwrap())
            .then([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: crate::RegisteredJobSpec::from(job("backup")),
            }]);

        let disable = TestCase::new(decider::<ChangeJobStateCommand>())
            .given(register.history())
            .when(ChangeJobStateCommand::new(
                JobId::parse("backup").unwrap(),
                JobEnabledState::Disabled,
            ))
            .then([JobEvent::JobStateChanged {
                id: "backup".to_string(),
                state: JobEnabledState::Disabled,
            }]);

        Timeline::new().given([register, disable]).then_stream(
            "backup",
            [
                JobEvent::JobRegistered {
                    id: "backup".to_string(),
                    spec: crate::RegisteredJobSpec::from(job("backup")),
                },
                JobEvent::JobStateChanged {
                    id: "backup".to_string(),
                    state: JobEnabledState::Disabled,
                },
            ],
        );
    }

    #[tokio::test]
    async fn run_updates_job_state() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = run(
            &store,
            &store,
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Disabled),
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobStateChanged {
                id: "backup".to_string(),
                state: JobEnabledState::Disabled,
            })
        );

        let snapshot = store
            .get_job(GetJobCommand {
                id: JobId::parse("backup").unwrap(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.payload.state, JobEnabledState::Disabled);

        let command_snapshot = store
            .read_command_snapshot::<ChangeJobStateState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(
                2,
                ChangeJobStateState::Present {
                    current: JobEnabledState::Disabled,
                },
            )
        );
    }
}
