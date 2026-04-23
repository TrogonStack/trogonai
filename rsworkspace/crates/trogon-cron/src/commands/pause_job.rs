use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, StreamCommand};

use super::JobState;
use crate::{
    JobId,
    events::{JobEvent, JobPaused},
};

#[derive(Debug, Clone)]
pub struct PauseJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    AlreadyPaused { id: JobId },
}

impl PauseJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl StreamCommand for PauseJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide for PauseJobCommand {
    type State = JobState;
    type Event = JobEvent;
    type DecideError = PauseJobDecisionError;

    fn decide(state: &JobState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobState::Missing => Err(PauseJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            JobState::Deleted => Err(PauseJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
            JobState::PresentDisabled => Err(PauseJobDecisionError::AlreadyPaused {
                id: command.stream_id().clone(),
            }),
            JobState::PresentEnabled => Ok(Decision::event(JobPaused {
                id: command.stream_id().to_string(),
            })),
        }
    }
}

impl CommandSnapshotPolicy for PauseJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, Decision, NonEmpty, decide,
        testing::{TestCase, Timeline, decider, expect_error},
    };

    use super::*;
    use crate::{
        AddJobCommand, Delivery, GetJobCommand, Job, JobAdded, JobEventStatus, JobHeaders, JobMessage, JobStatus,
        MessageContent, Schedule, mocks::MockCronStore,
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
    fn decides_pause_from_present_enabled_state() {
        let state = JobState::PresentEnabled;
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::event(JobPaused {
                id: "backup".to_string(),
            })
        );
    }

    #[test]
    fn rejects_pausing_already_paused_jobs() {
        let state = JobState::PresentDisabled;
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            PauseJobDecisionError::AlreadyPaused { .. }
        ));
    }

    #[test]
    fn rejects_pausing_missing_jobs() {
        let state = JobState::Missing;
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            PauseJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_pausing_deleted_jobs() {
        let state = JobState::Deleted;
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            PauseJobDecisionError::JobDeleted { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_pause_job_decider() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            })])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            })]);
    }

    #[test]
    fn given_when_then_supports_pause_job_failures() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([
                JobEvent::JobAdded(JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(job("backup")),
                }),
                JobEvent::JobPaused(JobPaused {
                    id: "backup".to_string(),
                }),
            ])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(PauseJobDecisionError::AlreadyPaused {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::new(decider::<AddJobCommand>())
            .given([])
            .when(AddJobCommand::new(job("backup")))
            .then([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            })]);

        let pause = TestCase::new(decider::<PauseJobCommand>())
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            })]);

        Timeline::new().given([register, pause]).then_stream(
            "backup",
            [
                JobEvent::JobAdded(JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(job("backup")),
                }),
                JobEvent::JobPaused(JobPaused {
                    id: "backup".to_string(),
                }),
            ],
        );
    }

    #[tokio::test]
    async fn run_pauses_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = CommandExecution::new(&store, &PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            }))
        );

        let job = store
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobEventStatus::Disabled);

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
