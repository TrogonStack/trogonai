use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshots, Decide, Decision, FrequencySnapshot, NonEmpty, OccPolicy,
    SnapshotRead, SnapshotWrite, StreamAppend, StreamCommand, StreamRead, StreamState, WritePrecondition,
};

use super::JobCommandState;
use crate::{
    JobId, JobSpec,
    events::{JobAdded, JobDetails, JobEvent},
};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    pub spec: JobSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecisionError {
    AlreadyExists { id: JobId },
    JobDeleted { id: JobId },
}

impl AddJobCommand {
    pub const fn new(spec: JobSpec) -> Self {
        Self { spec }
    }
}

impl StreamCommand for AddJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.spec.id
    }

    fn write_precondition(&self) -> Option<WritePrecondition> {
        Some(WritePrecondition::Require(StreamState::NoStream))
    }
}

impl Decide for AddJobCommand {
    type State = JobCommandState;
    type Event = JobEvent;
    type DecideError = AddJobDecisionError;

    fn decide(state: &JobCommandState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobCommandState::Missing => Ok(Decision::Event(NonEmpty::one(JobEvent::JobAdded(JobAdded {
                id: command.stream_id().to_string(),
                job: JobDetails::from(&command.spec),
            })))),
            JobCommandState::PresentEnabled | JobCommandState::PresentDisabled => {
                Err(AddJobDecisionError::AlreadyExists {
                    id: command.stream_id().clone(),
                })
            }
            JobCommandState::Deleted => Err(AddJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandSnapshots for AddJobCommand {
    type SnapshotPolicy = FrequencySnapshot;

    fn snapshot_policy() -> Self::SnapshotPolicy {
        super::command_snapshot_policy()
    }
}

pub async fn add_job<S, SErr>(
    store: &S,
    command: AddJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<AddJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<JobCommandState, JobId, Error = SErr>
        + SnapshotWrite<JobCommandState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .with_occ(occ)
        .with_snapshot(store)
        .execute()
        .await
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandFailure, Decision, NonEmpty, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        CronJob, DeliverySpec, GetJobCommand, JobEnabledState, JobHeaders, JobMessage, JobRemoved, MessageContent,
        ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::every(30).unwrap(),
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob::from((id.to_string(), JobDetails::from(job(id))))
    }

    #[test]
    fn decides_add_from_missing_state() {
        let state = JobCommandState::Missing;
        let command = AddJobCommand::new(job("backup"));

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
        let state = JobCommandState::PresentEnabled;
        let command = AddJobCommand::new(job("backup"));

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            AddJobDecisionError::AlreadyExists { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<AddJobCommand>())
            .given([])
            .when(AddJobCommand::new(job("backup")))
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
            .when(AddJobCommand::new(job("backup")))
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
            .when(AddJobCommand::new(job("backup")))
            .then(expect_error(AddJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = add_job(&store, AddJobCommand::new(job("backup")), None).await.unwrap();
        assert_eq!(outcome.next_expected_version, 1);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            }))
        );

        let stored_job = store
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<JobCommandState>(
                JobCommandState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        add_job(&store, AddJobCommand::new(job("backup")), None).await.unwrap();

        let error = add_job(&store, AddJobCommand::new(job("backup")), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecisionError::AlreadyExists { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_adding_deleted_job_id() {
        let store = MockCronStore::new();

        add_job(&store, AddJobCommand::new(job("backup")), None).await.unwrap();
        crate::remove_job(
            &store,
            crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
        .await
        .unwrap();

        let error = add_job(&store, AddJobCommand::new(job("backup")), None)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
