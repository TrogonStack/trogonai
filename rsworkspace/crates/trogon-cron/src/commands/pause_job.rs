use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::JobId;

#[derive(Debug, Clone)]
pub struct PauseJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseJobDecideError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    AlreadyPaused { id: JobId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for PauseJobDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PauseJobDecideError {}

impl PauseJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl Decider for PauseJobCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = PauseJobDecideError;
    type EvolveError = super::EvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(value) = state.state.as_ref() else {
            return Err(PauseJobDecideError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(PauseJobDecideError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(PauseJobDecideError::JobNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(PauseJobDecideError::JobDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(PauseJobDecideError::AlreadyPaused { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED => Ok(Decision::event(v1::JobEvent {
                event: Some(v1::JobPaused {}.into()),
            })),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(PauseJobDecideError::UnknownStateValue { value: 0 }),
        }
    }
}

impl CommandSnapshotPolicy for PauseJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;
    use trogon_decider::testing::{TestCase, Timeline};
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{CommandExecution, Events, run_task_immediately};

    use super::*;
    use crate::commands::domain::{Delivery, Job, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule};
    use crate::{AddJobCommand, GetJobCommand, JobEventStatus, mocks::MockCronStore};

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
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(v1::JobDetails::from(&job(id))),
                }
                .into(),
            ),
        }
    }

    fn paused() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobPaused {}.into()),
        }
    }

    fn removed() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        }
    }

    #[test]
    fn given_when_then_supports_pause_job_decider() {
        TestCase::<PauseJobCommand>::new()
            .given([added("backup")])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([paused()]);
    }

    #[test]
    fn given_when_then_supports_pause_job_failures() {
        TestCase::<PauseJobCommand>::new()
            .given([added("backup")])
            .given([paused()])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecideError::AlreadyPaused {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_missing_jobs() {
        TestCase::<PauseJobCommand>::new()
            .given_no_history()
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecideError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_deleted_jobs() {
        TestCase::<PauseJobCommand>::new()
            .given([added("backup")])
            .given([paused()])
            .given([removed()])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecideError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::<AddJobCommand>::new()
            .given_no_history()
            .when(AddJobCommand::new(job("backup")))
            .then([added("backup")]);

        let pause = TestCase::<PauseJobCommand>::new()
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then([paused()]);

        Timeline::new()
            .given([register, pause])
            .then_stream("backup", [added("backup"), paused()]);
    }

    #[tokio::test]
    async fn run_pauses_job() {
        let store = MockCronStore::new();
        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();

        let outcome = CommandExecution::new(&store, &PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.stream_position.get(), 2);
        assert_eq!(outcome.events, Events::one(paused()));

        let job = store
            .get_job(GetJobCommand::new(crate::JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobEventStatus::Disabled);

        let command_snapshot = store
            .read_command_snapshot::<state_v1::State>(
                state_v1::State::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
