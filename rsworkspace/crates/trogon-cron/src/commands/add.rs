use std::fmt;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandFailure, CommandInfraError, CommandOutcome,
    CommandState, CommandStreamState, Decide, Decision, EventStore, NonEmpty, OccPolicy,
    SnapshotState, SnapshotStore, SnapshotStoreConfig, Snapshots, StreamCommand, StreamState,
};

use crate::{
    JobId, JobSpec, ResolvedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventCodec, RegisteredJobSpec},
};

#[derive(Debug, Clone)]
pub struct RegisterJobCommand {
    id: JobId,
    job: ResolvedJobSpec,
}

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.register_job.v1.", None);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegisterJobState {
    Missing,
    Present,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterJobDecisionError {
    AlreadyRegistered { id: JobId },
    JobDeleted { id: JobId },
}

impl fmt::Display for RegisterJobDecisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered { id } => write!(f, "job '{id}' is already registered"),
            Self::JobDeleted { id } => {
                write!(f, "job '{id}' was deleted and cannot be registered again")
            }
        }
    }
}

impl std::error::Error for RegisterJobDecisionError {}

pub type RegisterJobResult = Result<
    CommandOutcome<JobEvent>,
    CommandFailure<RegisterJobDecisionError, CommandInfraError<CronError>>,
>;

impl RegisterJobCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CronError> {
        Ok(Self {
            id: spec.id.clone(),
            job: ResolvedJobSpec::try_from(&spec)?,
        })
    }

    pub fn job(&self) -> &ResolvedJobSpec {
        &self.job
    }
}

impl StreamCommand for RegisterJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn stream_state(&self) -> Option<CommandStreamState> {
        Some(CommandStreamState::Require(StreamState::NoStream))
    }
}

impl Decide<RegisterJobState, JobEvent> for RegisterJobCommand {
    type Error = RegisterJobDecisionError;

    fn decide(state: &RegisterJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RegisterJobState::Missing => {
                Ok(Decision::Event(NonEmpty::one(JobEvent::JobRegistered {
                    id: command.stream_id().to_string(),
                    spec: RegisteredJobSpec::from(command.job().spec()),
                })))
            }
            RegisterJobState::Present => Err(RegisterJobDecisionError::AlreadyRegistered {
                id: command.stream_id().clone(),
            }),
            RegisterJobState::Deleted => Err(RegisterJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandState for RegisterJobCommand {
    type State = RegisterJobState;
    type Event = JobEvent;
    type DomainError = RegisterJobDecisionError;

    fn initial_state() -> Self::State {
        RegisterJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => match state {
                RegisterJobState::Deleted => Ok(RegisterJobState::Deleted),
                RegisterJobState::Missing | RegisterJobState::Present => {
                    Ok(RegisterJobState::Present)
                }
            },
            JobEvent::JobRemoved { .. } => Ok(RegisterJobState::Deleted),
        }
    }
}

impl SnapshotState for RegisterJobCommand {
    type Snapshot = RegisterJobState;

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
        Some(*state)
    }
}

pub async fn run<E, S>(
    event_store: &E,
    snapshot_store: &S,
    command: RegisterJobCommand,
    occ: Option<OccPolicy>,
) -> RegisterJobResult
where
    E: EventStore<JobId, Error = CronError>,
    S: SnapshotStore<RegisterJobState, JobId, Error = CronError>,
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
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{
        CommandFailure, Decision, NonEmpty, Snapshot, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{DeliverySpec, GetJobCommand, JobEnabledState, ScheduleSpec, mocks::MockCronStore};

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn decides_registration_from_missing_state() {
        let state = RegisterJobState::Missing;
        let command = RegisterJobCommand::new(job("backup")).unwrap();

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }))
        );
    }

    #[test]
    fn rejects_registering_existing_job() {
        let state = RegisterJobState::Present;
        let command = RegisterJobCommand::new(job("backup")).unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RegisterJobDecisionError::AlreadyRegistered { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<RegisterJobCommand>())
            .given([])
            .when(RegisterJobCommand::new(job("backup")).unwrap())
            .then([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::new(decider::<RegisterJobCommand>())
            .given([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }])
            .when(RegisterJobCommand::new(job("backup")).unwrap())
            .then(expect_error(RegisterJobDecisionError::AlreadyRegistered {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn rejects_registering_deleted_job_ids() {
        TestCase::new(decider::<RegisterJobCommand>())
            .given([
                JobEvent::JobRegistered {
                    id: "backup".to_string(),
                    spec: RegisteredJobSpec::from(job("backup")),
                },
                JobEvent::JobRemoved {
                    id: "backup".to_string(),
                },
            ])
            .when(RegisterJobCommand::new(job("backup")).unwrap())
            .then(expect_error(RegisterJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = run(
            &store,
            &store,
            RegisterJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.next_expected_version, 1);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            })
        );

        let stored_job = store
            .get_job(GetJobCommand {
                id: JobId::parse("backup").unwrap(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<RegisterJobState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(1, RegisterJobState::Present)
        );
    }

    #[tokio::test]
    async fn run_rejects_registering_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        run(
            &store,
            &store,
            RegisterJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap();

        let error = run(
            &store,
            &store,
            RegisterJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(RegisterJobDecisionError::AlreadyRegistered { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_registering_deleted_job_id() {
        let store = MockCronStore::new();

        run(
            &store,
            &store,
            RegisterJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap();
        crate::remove_job(
            &store,
            &store,
            crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
        .await
        .unwrap();

        let error = run(
            &store,
            &store,
            RegisterJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(RegisterJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
