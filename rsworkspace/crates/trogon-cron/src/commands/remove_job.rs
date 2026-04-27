use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot};

use super::JobStateProtoError;
use crate::JobId;

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    InvalidState { source: JobStateProtoError },
}

impl RemoveJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl Decide for RemoveJobCommand {
    type StreamId = JobId;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = RemoveJobDecisionError;
    type EvolveError = JobStateProtoError;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self::Event>, Self::DecideError> {
        let state = state.state();
        match state {
            state_v1::StateValue::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            state_v1::StateValue::PresentEnabled | state_v1::StateValue::PresentDisabled => {
                let mut event = v1::JobEvent::new();
                event.set_job_removed(v1::JobRemoved::new());
                Ok(Decision::event(event))
            }
            state_v1::StateValue::Deleted => Err(RemoveJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
            _ => Err(RemoveJobDecisionError::InvalidState {
                source: JobStateProtoError::UnknownStateValue {
                    value: i32::from(state),
                },
            }),
        }
    }
}

impl CommandSnapshotPolicy for RemoveJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, NonEmpty,
        testing::{TestCase, decider},
    };

    use super::*;
    use crate::{
        Delivery, GetJobCommand, Job, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule, mocks::MockCronStore,
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
    fn given_when_then_supports_remove_job_decider() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([added("backup")])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then(trogon_eventsourcing::events![removed()]);
    }

    #[test]
    fn given_when_then_supports_remove_job_failures() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given_no_history()
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(RemoveJobDecisionError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_removing_deleted_job() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([added("backup")])
            .given([removed()])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(RemoveJobDecisionError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_removes_existing_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = CommandExecution::new(&store, &RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(outcome.events, NonEmpty::one(removed()));

        assert!(
            store
                .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
                .await
                .unwrap()
                .is_none()
        );

        let command_snapshot = store
            .read_command_snapshot::<state_v1::State>(
                state_v1::State::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
