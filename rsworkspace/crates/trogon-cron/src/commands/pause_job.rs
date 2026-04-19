use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandResult, CommandState, Decide, Decision, NonEmpty,
    OccPolicy, SnapshotStore, SnapshotStoreConfig, Snapshots, StreamAppend, StreamCommand,
    StreamRead,
};

use crate::{
    JobEnabledState, JobId, JobIdError,
    events::{JobEvent, JobEventCodec},
};

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.pause_job.v1.", None);

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

impl std::error::Error for PauseJobDecisionError {}

#[derive(Debug)]
pub enum PauseJobError {
    Decision(PauseJobDecisionError),
    InvalidAddEventId { id: String, source: JobIdError },
}

impl std::fmt::Display for PauseJobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decision(error) => write!(f, "{error}"),
            Self::InvalidAddEventId { id, .. } => {
                write!(f, "invalid job id '{id}' in add event while pausing job")
            }
        }
    }
}

impl std::error::Error for PauseJobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decision(error) => Some(error),
            Self::InvalidAddEventId { source, .. } => Some(source),
        }
    }
}

impl From<PauseJobDecisionError> for PauseJobError {
    fn from(value: PauseJobDecisionError) -> Self {
        Self::Decision(value)
    }
}

impl StreamCommand for PauseJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<PauseJobState, JobEvent> for PauseJobCommand {
    type Error = PauseJobDecisionError;

    fn decide(state: &PauseJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
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
            PauseJobState::Present { .. } => {
                Ok(Decision::Event(NonEmpty::one(JobEvent::JobPaused {
                    id: command.stream_id().to_string(),
                })))
            }
        }
    }
}

impl CommandState for PauseJobCommand {
    type State = PauseJobState;
    type Event = JobEvent;
    type DomainError = PauseJobError;

    fn initial_state() -> Self::State {
        PauseJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobAdded { id, job } => {
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
            JobEvent::JobPaused { .. } => match state {
                PauseJobState::Deleted => Ok(PauseJobState::Deleted),
                PauseJobState::Missing | PauseJobState::Present { .. } => {
                    Ok(PauseJobState::Present {
                        current: JobEnabledState::Disabled,
                    })
                }
            },
            JobEvent::JobResumed { .. } => match state {
                PauseJobState::Deleted => Ok(PauseJobState::Deleted),
                PauseJobState::Missing | PauseJobState::Present { .. } => {
                    Ok(PauseJobState::Present {
                        current: JobEnabledState::Enabled,
                    })
                }
            },
            JobEvent::JobRemoved { .. } => Ok(PauseJobState::Deleted),
        }
    }
}

pub async fn run<S, SErr>(
    store: &S,
    command: PauseJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<PauseJobState, JobEvent, PauseJobError, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotStore<PauseJobState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .codec(JobEventCodec)
        .occ(occ)
        .snapshots(Snapshots::new(store, SNAPSHOT_STORE_CONFIG, AlwaysSnapshot))
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
            Decision::Event(NonEmpty::one(JobEvent::JobPaused {
                id: "backup".to_string(),
            }))
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
            .given([JobEvent::JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            }])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobPaused {
                id: "backup".to_string(),
            }]);
    }

    #[test]
    fn given_when_then_supports_pause_job_failures() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([
                JobEvent::JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(job("backup")),
                },
                JobEvent::JobPaused {
                    id: "backup".to_string(),
                },
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
            .then([JobEvent::JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            }]);

        let pause = TestCase::new(decider::<PauseJobCommand>())
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobPaused {
                id: "backup".to_string(),
            }]);

        Timeline::new().given([register, pause]).then_stream(
            "backup",
            [
                JobEvent::JobAdded {
                    id: "backup".to_string(),
                    job: crate::JobDetails::from(job("backup")),
                },
                JobEvent::JobPaused {
                    id: "backup".to_string(),
                },
            ],
        );
    }

    #[tokio::test]
    async fn run_pauses_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = run(
            &store,
            PauseJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobPaused {
                id: "backup".to_string(),
            })
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
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(
                2,
                PauseJobState::Present {
                    current: JobEnabledState::Disabled,
                },
            )
        );
    }
}
