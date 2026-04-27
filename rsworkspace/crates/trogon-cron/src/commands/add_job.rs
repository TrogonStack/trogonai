use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot, StreamState};

use super::JobStateProtoError;
use super::domain::{Job, JobId};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecisionError {
    AlreadyExists { id: JobId },
    JobDeleted { id: JobId },
    InvalidState { source: JobStateProtoError },
}

impl AddJobCommand {
    pub const fn new(job: Job) -> Self {
        Self { job }
    }
}

impl Decide for AddJobCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = AddJobDecisionError;
    type EvolveError = JobStateProtoError;

    const REQUIRED_WRITE_PRECONDITION: Option<StreamState> = Some(StreamState::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        self.job.id.as_str()
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
            state_v1::StateValue::Missing => {
                let mut inner = v1::JobAdded::new();
                inner.set_job(v1::JobDetails::from(&command.job));
                let mut event = v1::JobEvent::new();
                event.set_job_added(inner);
                Ok(Decision::event(event))
            }
            state_v1::StateValue::PresentEnabled | state_v1::StateValue::PresentDisabled => {
                Err(AddJobDecisionError::AlreadyExists {
                    id: command.job.id.clone(),
                })
            }
            state_v1::StateValue::Deleted => Err(AddJobDecisionError::JobDeleted {
                id: command.job.id.clone(),
            }),
            _ => Err(AddJobDecisionError::InvalidState {
                source: JobStateProtoError::UnknownStateValue {
                    value: i32::from(state),
                },
            }),
        }
    }
}

impl CommandSnapshotPolicy for AddJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, CommandFailure, NonEmpty,
        testing::{TestCase, decider},
    };

    use super::*;
    use crate::commands::domain::{Delivery, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule};
    use crate::{
        CronJob, GetJobCommand, JobEventDelivery, JobEventSchedule, JobEventStatus,
        MessageContent as ReadMessageContent, MessageEnvelope, MessageHeaders, mocks::MockCronStore,
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
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: None,
                source: None,
            },
            message: MessageEnvelope {
                content: ReadMessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
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

    fn removed() -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        event.set_job_removed(v1::JobRemoved::new());
        event
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::new(decider::<AddJobCommand>())
            .given_no_history()
            .when(AddJobCommand::new(job("backup")))
            .then(trogon_eventsourcing::events![added("backup")]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::new(decider::<AddJobCommand>())
            .given([added("backup")])
            .when(AddJobCommand::new(job("backup")))
            .then_error(AddJobDecisionError::AlreadyExists {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::new(decider::<AddJobCommand>())
            .given([added("backup")])
            .given([removed()])
            .when(AddJobCommand::new(job("backup")))
            .then_error(AddJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 1);
        assert_eq!(outcome.events, NonEmpty::one(added("backup")));

        let stored_job = store
            .get_job(GetJobCommand::new(crate::JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<state_v1::State>(
                state_v1::State::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecisionError::AlreadyExists { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_adding_deleted_job_id() {
        let store = MockCronStore::new();

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        CommandExecution::new(&store, &crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecisionError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
