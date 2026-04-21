use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::snapshot::SnapshotSchema;
use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshots, Decide, Decision, FrequencySnapshot,
    NonEmpty, OccPolicy, SnapshotRead, SnapshotWrite, StreamAppend, StreamCommand, StreamRead,
};

use crate::{
    JobEnabledState, JobId,
    events::{JobAdded, JobEvent, JobPaused, JobRemoved, JobResumed},
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

impl StreamCommand for ResumeJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide for ResumeJobCommand {
    type State = ResumeJobState;
    type Event = JobEvent;
    type EvolveError = Infallible;
    type DecideError = ResumeJobDecisionError;

    fn initial_state() -> ResumeJobState {
        ResumeJobState::Missing
    }

    fn evolve(state: ResumeJobState, event: JobEvent) -> Result<ResumeJobState, Self::EvolveError> {
        match event {
            JobEvent::JobAdded(JobAdded { job, .. }) => {
                if matches!(state, ResumeJobState::Deleted) {
                    return Ok(ResumeJobState::Deleted);
                }
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

    fn decide(
        state: &ResumeJobState,
        command: &Self,
    ) -> Result<Decision<JobEvent>, Self::DecideError> {
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

impl CommandSnapshots for ResumeJobCommand {
    type SnapshotPolicy = FrequencySnapshot;

    fn snapshot_policy() -> Self::SnapshotPolicy {
        super::command_snapshot_policy()
    }
}

pub async fn resume_job<S, SErr>(
    store: &S,
    command: ResumeJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<ResumeJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<ResumeJobState, JobId, Error = SErr>
        + SnapshotWrite<ResumeJobState, JobId, Error = SErr>,
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
    use trogon_eventsourcing::{
        Decision, NonEmpty, decide,
        testing::{TestCase, Timeline, decider, expect_error},
    };

    use super::*;
    use crate::{
        AddJobCommand, DeliverySpec, GetJobCommand, JobEventState, JobHeaders, JobMessage, JobSpec,
        MessageContent, PauseJobCommand, ScheduleSpec, mocks::MockCronStore,
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

        let outcome = resume_job(
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
            .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.state, JobEventState::Enabled);

        let command_snapshot = store
            .read_command_snapshot::<ResumeJobState>(
                ResumeJobState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
