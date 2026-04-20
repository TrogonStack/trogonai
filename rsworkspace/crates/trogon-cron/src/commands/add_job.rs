use std::fmt;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandResult, CommandState, CommandStreamState, Decide,
    Decision, NonEmpty, OccPolicy, SnapshotRead, SnapshotSchema, SnapshotWrite, Snapshots,
    StreamAppend, StreamCommand, StreamRead, StreamState,
};

use crate::{
    CronJob, JobId, JobSpec, ResolvedJobSpec,
    error::CronError,
    events::{JobAdded, JobDetails, JobEvent, JobEventCodec, JobPaused, JobRemoved, JobResumed},
};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    id: JobId,
    spec: JobSpec,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AddJobState {
    Missing,
    Present,
    Deleted,
}

impl SnapshotSchema for AddJobState {
    const NAMESPACE: &'static str = "cron.command";
    const SCHEMA_SEGMENT: &'static str = "add_job.v1";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecisionError {
    AlreadyExists { id: JobId },
    JobDeleted { id: JobId },
}

impl fmt::Display for AddJobDecisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists { id } => write!(f, "job '{id}' already exists"),
            Self::JobDeleted { id } => {
                write!(f, "job '{id}' was deleted and cannot be added again")
            }
        }
    }
}

impl std::error::Error for AddJobDecisionError {}

impl AddJobCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CronError> {
        ResolvedJobSpec::try_from(&CronJob::from((
            spec.id.to_string(),
            JobDetails::from(&spec),
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
            AddJobState::Missing => Ok(Decision::Event(NonEmpty::one(JobEvent::JobAdded(
                JobAdded {
                    id: command.stream_id().to_string(),
                    job: JobDetails::from(command.spec()),
                },
            )))),
            AddJobState::Present => Err(AddJobDecisionError::AlreadyExists {
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
            JobEvent::JobAdded(JobAdded { .. })
            | JobEvent::JobPaused(JobPaused { .. })
            | JobEvent::JobResumed(JobResumed { .. }) => match state {
                AddJobState::Deleted => Ok(AddJobState::Deleted),
                AddJobState::Missing | AddJobState::Present => Ok(AddJobState::Present),
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Ok(AddJobState::Deleted),
        }
    }
}

pub async fn run<S, SErr>(
    store: &S,
    command: AddJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<AddJobState, JobEvent, AddJobDecisionError, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<AddJobState, JobId, Error = SErr>
        + SnapshotWrite<AddJobState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .codec(JobEventCodec)
        .occ(occ)
        .snapshots(Snapshots::new(
            store,
            AddJobState::snapshot_store_config(),
            AlwaysSnapshot,
        ))
        .execute()
        .await
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{
        CommandFailure, Decision, NonEmpty, Snapshot, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        CronJob, DeliverySpec, GetJobCommand, JobEnabledState, MessageContent, MessageHeaders,
        ScheduleSpec, mocks::MockCronStore,
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
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), JobDetails::from(job(id))))
    }

    #[test]
    fn decides_add_from_missing_state() {
        let state = AddJobState::Missing;
        let command = AddJobCommand::new(job("backup")).unwrap();

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            })))
        );
    }

    #[test]
    fn rejects_adding_existing_job() {
        let state = AddJobState::Present;
        let command = AddJobCommand::new(job("backup")).unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            AddJobDecisionError::AlreadyExists { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<AddJobCommand>())
            .given([])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            })]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::new(decider::<AddJobCommand>())
            .given([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            })])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then(expect_error(AddJobDecisionError::AlreadyExists {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::new(decider::<AddJobCommand>())
            .given([
                JobEvent::JobAdded(JobAdded {
                    id: "backup".to_string(),
                    job: JobDetails::from(job("backup")),
                }),
                JobEvent::JobRemoved(JobRemoved {
                    id: "backup".to_string(),
                }),
            ])
            .when(AddJobCommand::new(job("backup")).unwrap())
            .then(expect_error(AddJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = run(&store, AddJobCommand::new(job("backup")).unwrap(), None)
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 1);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            }))
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
                AddJobState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(command_snapshot, Snapshot::new(1, AddJobState::Present));
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        run(&store, AddJobCommand::new(job("backup")).unwrap(), None)
            .await
            .unwrap();

        let error = run(&store, AddJobCommand::new(job("backup")).unwrap(), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(AddJobDecisionError::AlreadyExists { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_adding_deleted_job_id() {
        let store = MockCronStore::new();

        run(&store, AddJobCommand::new(job("backup")).unwrap(), None)
            .await
            .unwrap();
        crate::remove_job(
            &store,
            crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
        .await
        .unwrap();

        let error = run(&store, AddJobCommand::new(job("backup")).unwrap(), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Domain(AddJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
