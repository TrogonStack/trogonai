use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, StreamCommand};

use super::JobState;
use crate::{
    JobId,
    events::{JobEvent, JobResumed},
};

#[derive(Debug, Clone)]
pub struct ResumeJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    AlreadyActive { id: JobId },
}

impl ResumeJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl StreamCommand for ResumeJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide for ResumeJobCommand {
    type State = JobState;
    type Event = JobEvent;
    type DecideError = ResumeJobDecisionError;

    fn decide(state: &JobState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobState::Missing => Err(ResumeJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            JobState::Deleted => Err(ResumeJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
            JobState::PresentEnabled => Err(ResumeJobDecisionError::AlreadyActive {
                id: command.stream_id().clone(),
            }),
            JobState::PresentDisabled => Ok(Decision::event(JobResumed {
                id: command.stream_id().to_string(),
            })),
        }
    }
}

impl CommandSnapshotPolicy for ResumeJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, NonEmpty,
        testing::{TestCase, Timeline, decider, expect_error},
    };

    use super::*;
    use crate::{
        AddJobCommand, Delivery, GetJobCommand, Job, JobAdded, JobEventStatus, JobHeaders, JobMessage, JobPaused,
        JobStatus, MessageContent, PauseJobCommand, Schedule, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn active_job(id: &str) -> Job {
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

    fn paused_job(id: &str) -> Job {
        let mut job = active_job(id);
        job.status = JobStatus::Disabled;
        job
    }

    #[test]
    fn given_when_then_supports_resume_job_decider() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            }])
            .given([JobPaused {
                id: "backup".to_string(),
            }])
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![JobResumed {
                id: "backup".to_string(),
            }]);
    }

    #[test]
    fn given_when_then_supports_resume_job_failures() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            }])
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(ResumeJobDecisionError::AlreadyActive {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn given_when_then_rejects_resuming_missing_jobs() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given_no_history()
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(ResumeJobDecisionError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn given_when_then_rejects_resuming_deleted_jobs() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given([JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            }])
            .given([JobPaused {
                id: "backup".to_string(),
            }])
            .given([crate::JobRemoved {
                id: "backup".to_string(),
            }])
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(ResumeJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::new(decider::<AddJobCommand>())
            .given_no_history()
            .when(AddJobCommand::new(active_job("backup")))
            .then(trogon_eventsourcing::events![JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            }]);

        let pause = TestCase::new(decider::<PauseJobCommand>())
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![JobPaused {
                id: "backup".to_string(),
            }]);

        let resume = TestCase::new(decider::<ResumeJobCommand>())
            .given(pause.history())
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![JobResumed {
                id: "backup".to_string(),
            }]);

        Timeline::new().given([register, pause, resume]).then_stream(
            "backup",
            trogon_eventsourcing::events![
                JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(active_job("backup")),
                },
                JobPaused {
                    id: "backup".to_string(),
                },
                JobResumed {
                    id: "backup".to_string(),
                },
            ],
        );
    }

    #[tokio::test]
    async fn run_resumes_job() {
        let store = MockCronStore::new();
        store.seed_job(paused_job("backup"));

        let outcome = CommandExecution::new(&store, &ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(
                JobResumed {
                    id: "backup".to_string(),
                }
                .into()
            )
        );

        let job = store
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobEventStatus::Enabled);

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
