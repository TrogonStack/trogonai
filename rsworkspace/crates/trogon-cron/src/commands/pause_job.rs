use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot};

use super::JobStateProtoError;
use super::domain::JobId;

#[derive(Debug, Clone)]
pub struct PauseJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    AlreadyPaused { id: JobId },
    InvalidState { source: JobStateProtoError },
}

impl PauseJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl Decide for PauseJobCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = PauseJobDecisionError;
    type EvolveError = JobStateProtoError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self::Event>, Self::DecideError> {
        let state = state.state();
        match state {
            state_v1::StateValue::Missing => Err(PauseJobDecisionError::JobNotFound { id: command.id.clone() }),
            state_v1::StateValue::Deleted => Err(PauseJobDecisionError::JobDeleted { id: command.id.clone() }),
            state_v1::StateValue::PresentDisabled => {
                Err(PauseJobDecisionError::AlreadyPaused { id: command.id.clone() })
            }
            state_v1::StateValue::PresentEnabled => {
                let mut event = v1::JobEvent::new();
                event.set_job_paused(v1::JobPaused::new());
                Ok(Decision::event(event))
            }
            _ => Err(PauseJobDecisionError::InvalidState {
                source: JobStateProtoError::UnknownStateValue {
                    value: i32::from(state),
                },
            }),
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
        CommandExecution, NonEmpty, run_task_immediately,
        testing::{TestCase, Timeline, decider},
    };

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
        let mut inner = v1::JobAdded::new();
        inner.set_job(v1::JobDetails::from(&job(id)));
        let mut event = v1::JobEvent::new();
        event.set_job_added(inner);
        event
    }

    fn paused() -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_paused(v1::JobPaused::new());
        event
    }

    fn removed() -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_removed(v1::JobRemoved::new());
        event
    }

    #[test]
    fn given_when_then_supports_pause_job_decider() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([added("backup")])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![paused()]);
    }

    #[test]
    fn given_when_then_supports_pause_job_failures() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([added("backup")])
            .given([paused()])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecisionError::AlreadyPaused {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_missing_jobs() {
        TestCase::new(decider::<PauseJobCommand>())
            .given_no_history()
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecisionError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_pausing_deleted_jobs() {
        TestCase::new(decider::<PauseJobCommand>())
            .given([added("backup")])
            .given([paused()])
            .given([removed()])
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(PauseJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn timeline_matches_cases_by_command_stream() {
        let register = TestCase::new(decider::<AddJobCommand>())
            .given_no_history()
            .when(AddJobCommand::new(job("backup")))
            .then(trogon_eventsourcing::events![added("backup")]);

        let pause = TestCase::new(decider::<PauseJobCommand>())
            .given(register.history())
            .when(PauseJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![paused()]);

        Timeline::new()
            .given([register, pause])
            .then_stream("backup", trogon_eventsourcing::events![added("backup"), paused()]);
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
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(outcome.events, NonEmpty::one(paused()));

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
