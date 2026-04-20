use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandResult, CommandSnapshots, CommandState, Decide,
    Decision, NonEmpty, OccPolicy, SnapshotRead, SnapshotSchema, SnapshotWrite, StreamAppend,
    StreamCommand, StreamRead,
};

use crate::{
    JobEnabledState, JobId, JobIdError,
    events::{JobAdded, JobEvent, JobEventCodec, JobPaused, JobRemoved, JobResumed},
};

#[derive(Debug, Clone)]
pub struct ResumeJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResumeJobState {
    Missing,
    Present { current: JobEnabledState },
    Deleted,
}

impl SnapshotSchema for ResumeJobState {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.resume_job.v1.";
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

impl std::fmt::Display for ResumeJobDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for resume '{id}'"),
            Self::JobDeleted { id } => write!(f, "job '{id}' was deleted and cannot be resumed"),
            Self::AlreadyActive { id } => write!(f, "job '{id}' is already active"),
        }
    }
}

impl std::error::Error for ResumeJobDecisionError {}

#[derive(Debug)]
pub enum ResumeJobError {
    Decision(ResumeJobDecisionError),
    InvalidAddEventId { id: String, source: JobIdError },
}

impl std::fmt::Display for ResumeJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decision(error) => write!(f, "{error}"),
            Self::InvalidAddEventId { id, .. } => {
                write!(f, "invalid job id '{id}' in add event while resuming job")
            }
        }
    }
}

impl std::error::Error for ResumeJobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decision(error) => Some(error),
            Self::InvalidAddEventId { source, .. } => Some(source),
        }
    }
}

impl From<ResumeJobDecisionError> for ResumeJobError {
    fn from(value: ResumeJobDecisionError) -> Self {
        Self::Decision(value)
    }
}

impl StreamCommand for ResumeJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<ResumeJobState, JobEvent> for ResumeJobCommand {
    type Error = ResumeJobDecisionError;

    fn decide(state: &ResumeJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            ResumeJobState::Missing => Err(ResumeJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            ResumeJobState::Deleted => Err(ResumeJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
            ResumeJobState::Present {
                current: JobEnabledState::Enabled,
            } => Err(ResumeJobDecisionError::AlreadyActive {
                id: command.stream_id().clone(),
            }),
            ResumeJobState::Present { .. } => Ok(Decision::Event(NonEmpty::one(
                JobEvent::JobResumed(JobResumed {
                    id: command.stream_id().to_string(),
                }),
            ))),
        }
    }
}

impl CommandState for ResumeJobCommand {
    type State = ResumeJobState;
    type Event = JobEvent;
    type DomainError = ResumeJobError;

    fn initial_state() -> Self::State {
        ResumeJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobAdded(JobAdded { id, job }) => {
                if matches!(state, ResumeJobState::Deleted) {
                    return Ok(ResumeJobState::Deleted);
                }
                JobId::parse(&id).map_err(|source| ResumeJobError::InvalidAddEventId {
                    id: id.clone(),
                    source,
                })?;
                Ok(ResumeJobState::Present {
                    current: job.state.into(),
                })
            }
            JobEvent::JobPaused(JobPaused { .. }) => match state {
                ResumeJobState::Deleted => Ok(ResumeJobState::Deleted),
                ResumeJobState::Missing | ResumeJobState::Present { .. } => {
                    Ok(ResumeJobState::Present {
                        current: JobEnabledState::Disabled,
                    })
                }
            },
            JobEvent::JobResumed(JobResumed { .. }) => match state {
                ResumeJobState::Deleted => Ok(ResumeJobState::Deleted),
                ResumeJobState::Missing | ResumeJobState::Present { .. } => {
                    Ok(ResumeJobState::Present {
                        current: JobEnabledState::Enabled,
                    })
                }
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Ok(ResumeJobState::Deleted),
        }
    }
}

impl CommandSnapshots for ResumeJobCommand {
    type SnapshotPolicy = AlwaysSnapshot;

    fn snapshot_policy() -> Self::SnapshotPolicy {
        AlwaysSnapshot
    }
}

pub async fn run<S, SErr>(
    store: &S,
    command: ResumeJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<ResumeJobState, JobEvent, ResumeJobError, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<ResumeJobState, JobId, Error = SErr>
        + SnapshotWrite<ResumeJobState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .with_codec(JobEventCodec)
        .with_occ(occ)
        .with_snapshot(store)
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
        AddJobCommand, DeliverySpec, GetJobCommand, JobEventState, JobSpec, MessageContent,
        MessageHeaders, PauseJobCommand, ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn active_job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        }
    }

    fn paused_job(id: &str) -> JobSpec {
        let mut job = active_job(id);
        job.state = JobEnabledState::Disabled;
        job
    }

    #[test]
    fn decides_resume_from_present_disabled_state() {
        let state = ResumeJobState::Present {
            current: JobEnabledState::Disabled,
        };
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobResumed(JobResumed {
                id: "backup".to_string(),
            })))
        );
    }

    #[test]
    fn rejects_resuming_active_jobs() {
        let state = ResumeJobState::Present {
            current: JobEnabledState::Enabled,
        };
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ResumeJobDecisionError::AlreadyActive { .. }
        ));
    }

    #[test]
    fn rejects_resuming_missing_jobs() {
        let state = ResumeJobState::Missing;
        let command = ResumeJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ResumeJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_resuming_deleted_jobs() {
        let state = ResumeJobState::Deleted;
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
            .when(AddJobCommand::new(active_job("backup")).unwrap())
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

        Timeline::new()
            .given([register, pause, resume])
            .then_stream(
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

        let outcome = run(
            &store,
            ResumeJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
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
            .get_job(GetJobCommand {
                id: "backup".to_string(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.state, JobEventState::Enabled);

        let command_snapshot = store
            .read_command_snapshot::<ResumeJobState>(
                ResumeJobState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(
                2,
                ResumeJobState::Present {
                    current: JobEnabledState::Enabled,
                },
            )
        );
    }
}
