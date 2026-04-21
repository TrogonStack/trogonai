use serde::{Deserialize, Serialize};
use trogon_eventsourcing::snapshot::SnapshotSchema;
use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshots, CommandState, Decide, Decision,
    FrequencySnapshot, NonEmpty, OccPolicy, SnapshotRead, SnapshotWrite, StreamAppend,
    StreamCommand, StreamRead,
};

use crate::{
    JobEnabledState, JobId, JobIdError,
    events::{JobAdded, JobEvent, JobPaused, JobRemoved, JobResumed},
};

#[derive(Debug, Clone)]
pub struct PauseJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PauseJobState {
    Missing,
    Present { current: JobEnabledState },
    Deleted,
}

impl SnapshotSchema for PauseJobState {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.pause_job.v1.";
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

impl std::fmt::Display for PauseJobDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for pause '{id}'"),
            Self::JobDeleted { id } => write!(f, "job '{id}' was deleted and cannot be paused"),
            Self::AlreadyPaused { id } => write!(f, "job '{id}' is already paused"),
        }
    }
}

#[derive(Debug)]
pub enum PauseJobError {
    InvalidAddEventId { id: String, source: JobIdError },
}

impl std::fmt::Display for PauseJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAddEventId { id, .. } => {
                write!(f, "invalid job id '{id}' in add event while pausing job")
            }
        }
    }
}

impl std::error::Error for PauseJobError {}

impl StreamCommand for PauseJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<PauseJobState, JobEvent> for PauseJobCommand {
    type EvolveError = PauseJobError;
    type DecideError = PauseJobDecisionError;

    fn initial_state() -> PauseJobState {
        PauseJobState::Missing
    }

    fn evolve(state: PauseJobState, event: JobEvent) -> Result<PauseJobState, Self::EvolveError> {
        match event {
            JobEvent::JobAdded(JobAdded { id, job }) => {
                if matches!(state, PauseJobState::Deleted) {
                    return Ok(PauseJobState::Deleted);
                }
                JobId::parse(&id).map_err(|source| PauseJobError::InvalidAddEventId {
                    id: id.clone(),
                    source,
                })?;
                Ok(PauseJobState::Present {
                    current: job.state.into(),
                })
            }
            JobEvent::JobPaused(JobPaused { .. }) => match state {
                PauseJobState::Deleted => Ok(PauseJobState::Deleted),
                PauseJobState::Missing | PauseJobState::Present { .. } => {
                    Ok(PauseJobState::Present {
                        current: JobEnabledState::Disabled,
                    })
                }
            },
            JobEvent::JobResumed(JobResumed { .. }) => match state {
                PauseJobState::Deleted => Ok(PauseJobState::Deleted),
                PauseJobState::Missing | PauseJobState::Present { .. } => {
                    Ok(PauseJobState::Present {
                        current: JobEnabledState::Enabled,
                    })
                }
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Ok(PauseJobState::Deleted),
        }
    }

    fn decide(
        state: &PauseJobState,
        command: &Self,
    ) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            PauseJobState::Missing => Err(PauseJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            PauseJobState::Deleted => Err(PauseJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
            PauseJobState::Present {
                current: JobEnabledState::Disabled,
            } => Err(PauseJobDecisionError::AlreadyPaused {
                id: command.stream_id().clone(),
            }),
            PauseJobState::Present { .. } => Ok(Decision::Event(NonEmpty::one(
                JobEvent::JobPaused(JobPaused {
                    id: command.stream_id().to_string(),
                }),
            ))),
        }
    }
}

impl CommandState for PauseJobCommand {
    type State = PauseJobState;
    type Event = JobEvent;
}

impl CommandSnapshots for PauseJobCommand {
    type SnapshotPolicy = FrequencySnapshot;

    fn snapshot_policy() -> Self::SnapshotPolicy {
        super::command_snapshot_policy()
    }
}

pub async fn pause_job<S, SErr>(
    store: &S,
    command: PauseJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<PauseJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<PauseJobState, JobId, Error = SErr>
        + SnapshotWrite<PauseJobState, JobId, Error = SErr>,
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
        AddJobCommand, DeliverySpec, GetJobCommand, JobEventState, JobSpec, MessageContent,
        MessageHeaders, ScheduleSpec, mocks::MockCronStore,
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

    #[test]
    fn decides_pause_from_present_enabled_state() {
        let state = PauseJobState::Present {
            current: JobEnabledState::Enabled,
        };
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobPaused(JobPaused {
                id: "backup".to_string(),
            })))
        );
    }

    #[test]
    fn rejects_pausing_already_paused_jobs() {
        let state = PauseJobState::Present {
            current: JobEnabledState::Disabled,
        };
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            PauseJobDecisionError::AlreadyPaused { .. }
        ));
    }

    #[test]
    fn rejects_pausing_missing_jobs() {
        let state = PauseJobState::Missing;
        let command = PauseJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            PauseJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_pausing_deleted_jobs() {
        let state = PauseJobState::Deleted;
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
            .when(AddJobCommand::new(job("backup")).unwrap())
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

        let outcome = pause_job(
            &store,
            PauseJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
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
            .get_job(GetJobCommand {
                id: "backup".to_string(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.state, JobEventState::Disabled);

        let command_snapshot = store
            .read_command_snapshot::<PauseJobState>(
                PauseJobState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
