use std::fmt;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandResult, CommandState, CommandStreamState, Decide,
    Decision, NonEmpty, OccPolicy, SnapshotStore, SnapshotStoreConfig, Snapshots, StreamAppend,
    StreamCommand, StreamRead, StreamState,
};

use crate::{
    CronJob, JobId, JobSpec, ResolvedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventCodec, RegisteredJobSpec},
};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    id: JobId,
    spec: JobSpec,
}

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.add_job.v1.", None);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AddJobState {
    Missing,
    Present,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecisionError {
    AlreadyRegistered { id: JobId },
    JobDeleted { id: JobId },
}

impl fmt::Display for AddJobDecisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered { id } => write!(f, "job '{id}' is already registered"),
            Self::JobDeleted { id } => {
                write!(f, "job '{id}' was deleted and cannot be registered again")
            }
        }
    }
}

impl std::error::Error for AddJobDecisionError {}

impl AddJobCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CronError> {
        ResolvedJobSpec::try_from(&CronJob::from((
            spec.id.to_string(),
            RegisteredJobSpec::from(&spec),
        )))?;

        Ok(Self {
            id: spec.id.clone(),
            spec,
        })
    }

    pub fn spec(&self) -> &JobSpec {
        &self.spec
    }
}

impl StreamCommand for AddJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn stream_state(&self) -> Option<CommandStreamState> {
        Some(CommandStreamState::Require(StreamState::NoStream))
    }
}

impl Decide<AddJobState, JobEvent> for AddJobCommand {
    type Error = AddJobDecisionError;

    fn decide(state: &AddJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            AddJobState::Missing => Ok(Decision::Event(NonEmpty::one(JobEvent::JobRegistered {
                id: command.stream_id().to_string(),
                spec: RegisteredJobSpec::from(command.spec()),
            }))),
            AddJobState::Present => Err(AddJobDecisionError::AlreadyRegistered {
                id: command.stream_id().clone(),
            }),
            AddJobState::Deleted => Err(AddJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandState for AddJobCommand {
    type State = AddJobState;
    type Event = JobEvent;
    type DomainError = AddJobDecisionError;

    fn initial_state() -> Self::State {
        AddJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { .. }
            | JobEvent::JobPaused { .. }
            | JobEvent::JobResumed { .. } => match state {
                AddJobState::Deleted => Ok(AddJobState::Deleted),
                AddJobState::Missing | AddJobState::Present => Ok(AddJobState::Present),
            },
            JobEvent::JobRemoved { .. } => Ok(AddJobState::Deleted),
        }
    }
}

pub async fn run<E, S>(
    event_store: &E,
    snapshot_store: &S,
    command: AddJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<AddJobState, JobEvent, AddJobDecisionError, CronError>
where
    E: StreamRead<JobId, Error = CronError> + StreamAppend<JobId, Error = CronError>,
    S: SnapshotStore<AddJobState, JobId, Error = CronError>,
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
    use crate::{
        CronJob, DeliverySpec, GetJobCommand, JobEnabledState, ScheduleSpec, mocks::MockCronStore,
    };

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

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), RegisteredJobSpec::from(job(id))))
    }

    #[test]
    fn decides_registration_from_missing_state() {
        let state = AddJobState::Missing;
        let command = AddJobCommand::new(job("backup")).unwrap();

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
        let state = AddJobState::Present;
        let command = AddJobCommand::new(job("backup")).unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            AddJobDecisionError::AlreadyRegistered { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<AddJobCommand>())
            .given([])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::new(decider::<AddJobCommand>())
            .given([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then(expect_error(AddJobDecisionError::AlreadyRegistered {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn rejects_registering_deleted_job_ids() {
        TestCase::new(decider::<AddJobCommand>())
            .given([
                JobEvent::JobRegistered {
                    id: "backup".to_string(),
                    spec: RegisteredJobSpec::from(job("backup")),
                },
                JobEvent::JobRemoved {
                    id: "backup".to_string(),
                },
            ])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then(expect_error(AddJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = run(
            &store,
            &store,
            AddJobCommand::new(job("backup")).unwrap(),
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
                id: "backup".to_string(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<AddJobState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(command_snapshot, Snapshot::new(1, AddJobState::Present));
    }

    #[tokio::test]
    async fn run_rejects_registering_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        run(
            &store,
            &store,
            AddJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap();

        let error = run(
            &store,
            &store,
            AddJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(AddJobDecisionError::AlreadyRegistered { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_registering_deleted_job_id() {
        let store = MockCronStore::new();

        run(
            &store,
            &store,
            AddJobCommand::new(job("backup")).unwrap(),
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
            AddJobCommand::new(job("backup")).unwrap(),
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(AddJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
