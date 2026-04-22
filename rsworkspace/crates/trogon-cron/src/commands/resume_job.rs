use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, OccPolicy,
    SnapshotRead, SnapshotWrite, StreamAppend, StreamCommand, StreamRead,
};

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

pub async fn resume_job<S, SErr>(
    store: &S,
    command: ResumeJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<ResumeJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<JobState, JobId, Error = SErr>
        + SnapshotWrite<JobState, JobId, Error = SErr>,
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
        Decision, NonEmpty, decide,
        testing::{TestCase, Timeline, decider, expect_error},
    };

    use super::*;
    use crate::{
        AddJobCommand, DeliverySpec, GetJobCommand, JobAdded, JobEnabledState, JobEventState, JobHeaders, JobMessage,
        JobPaused, JobSpec, MessageContent, PauseJobCommand, ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn active_job(id: &str) -> JobSpec {
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

    fn paused_job(id: &str) -> JobSpec {
        let mut job = active_job(id);
        job.state = JobEnabledState::Disabled;
        job
    }

    #[test]
    fn decides_resume_from_present_disabled_state() {
        let state = JobState::PresentDisabled;
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::event(JobResumed {
                id: "backup".to_string(),
            })
        );
    }

    #[test]
    fn rejects_resuming_active_jobs() {
        let state = JobState::PresentEnabled;
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ResumeJobDecisionError::AlreadyActive { .. }
        ));
    }

    #[test]
    fn rejects_resuming_missing_jobs() {
        let state = JobState::Missing;
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ResumeJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_resuming_deleted_jobs() {
        let state = JobState::Deleted;
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ResumeJobDecisionError::JobDeleted { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_resume_job_decider() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given([
                JobEvent::JobAdded(JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(active_job("backup")),
                }),
                JobEvent::JobPaused(JobPaused {
                    id: "backup".to_string(),
                }),
            ])
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobResumed(JobResumed {
                id: "backup".to_string(),
            })]);
    }

    #[test]
    fn given_when_then_supports_resume_job_failures() {
        TestCase::new(decider::<ResumeJobCommand>())
            .given([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            })])
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(ResumeJobDecisionError::AlreadyActive {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::new(decider::<AddJobCommand>())
            .given([])
            .when(AddJobCommand::new(active_job("backup")))
            .then([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(active_job("backup")),
            })]);

        let pause = TestCase::new(decider::<PauseJobCommand>())
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            })]);

        let resume = TestCase::new(decider::<ResumeJobCommand>())
            .given(pause.history())
            .when(ResumeJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobResumed(JobResumed {
                id: "backup".to_string(),
            })]);

        Timeline::new().given([register, pause, resume]).then_stream(
            "backup",
            [
                JobEvent::JobAdded(JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(active_job("backup")),
                }),
                JobEvent::JobPaused(JobPaused {
                    id: "backup".to_string(),
                }),
                JobEvent::JobResumed(JobResumed {
                    id: "backup".to_string(),
                }),
            ],
        );
    }

    #[tokio::test]
    async fn run_resumes_job() {
        let store = MockCronStore::new();
        store.seed_job(paused_job("backup"));

        let outcome = resume_job(&store, ResumeJobCommand::new(JobId::parse("backup").unwrap()), None)
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobResumed(JobResumed {
                id: "backup".to_string(),
            }))
        );

        let job = store
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.state, JobEventState::Enabled);

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
