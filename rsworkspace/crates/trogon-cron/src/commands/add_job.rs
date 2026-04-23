use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, StreamCommand, StreamState};

use super::JobState;
use crate::{
    Job, JobId,
    events::{JobAdded, JobDetails, JobEvent},
};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecisionError {
    AlreadyExists { id: JobId },
    JobDeleted { id: JobId },
}

impl AddJobCommand {
    pub const fn new(job: Job) -> Self {
        Self { job }
    }
}

impl StreamCommand for AddJobCommand {
    type StreamId = JobId;
    const REQUIRED_WRITE_PRECONDITION: Option<StreamState> = Some(StreamState::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        &self.job.id
    }
}

impl Decide for AddJobCommand {
    type State = JobState;
    type Event = JobEvent;
    type DecideError = AddJobDecisionError;

    fn decide(state: &JobState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobState::Missing => Ok(Decision::event(JobAdded {
                id: command.stream_id().to_string(),
                job: JobDetails::from(&command.job),
            })),
            JobState::PresentEnabled | JobState::PresentDisabled => Err(AddJobDecisionError::AlreadyExists {
                id: command.stream_id().clone(),
            }),
            JobState::Deleted => Err(AddJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for AddJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, CommandFailure, NonEmpty,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        CronJob, Delivery, GetJobCommand, JobHeaders, JobMessage, JobRemoved, JobStatus, MessageContent, Schedule,
        mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: Schedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
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
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<AddJobCommand>())
            .given_no_history()
            .when(AddJobCommand::new(job("backup")))
            .then(trogon_eventsourcing::events![JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            }]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::new(decider::<AddJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            }])
            .when(AddJobCommand::new(job("backup")))
            .then(expect_error(AddJobDecisionError::AlreadyExists {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::new(decider::<AddJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: JobDetails::from(job("backup")),
            }])
            .given([JobRemoved {
                id: "backup".to_string(),
            }])
            .when(AddJobCommand::new(job("backup")))
            .then(expect_error(AddJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 1);
        assert_eq!(
            outcome.events,
            NonEmpty::one(
                JobAdded {
                    id: "backup".to_string(),
                    job: JobDetails::from(job("backup")),
                }
                .into()
            )
        );

        let stored_job = store
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
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

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        CommandExecution::new(&store, &crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
