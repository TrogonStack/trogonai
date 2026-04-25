use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, StreamCommand};

use super::JobState;
use crate::{JobEvent, JobId, JobRemoved};

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
}

impl RemoveJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl StreamCommand for RemoveJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide for RemoveJobCommand {
    type State = JobState;
    type Event = JobEvent;
    type DecideError = RemoveJobDecisionError;

    fn decide(state: &JobState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobState::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            JobState::PresentEnabled | JobState::PresentDisabled => Ok(Decision::event(JobRemoved {
                id: command.stream_id().to_string(),
            })),
            JobState::Deleted => Err(RemoveJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RemoveJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, NonEmpty,
        testing::{TestCase, decider},
    };

    use super::*;
    use crate::{
        Delivery, GetJobCommand, Job, JobAdded, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule,
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

    #[test]
    fn given_when_then_supports_remove_job_decider() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            }])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![JobRemoved {
                id: "backup".to_string(),
            }]);
    }

    #[test]
    fn given_when_then_supports_remove_job_failures() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given_no_history()
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(RemoveJobDecisionError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_removing_deleted_job() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            }])
            .given([JobRemoved {
                id: "backup".to_string(),
            }])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(RemoveJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_removes_existing_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = CommandExecution::new(&store, &RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(
                JobRemoved {
                    id: "backup".to_string(),
                }
                .into()
            )
        );

        assert!(
            store
                .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
                .await
                .unwrap()
                .is_none()
        );

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
