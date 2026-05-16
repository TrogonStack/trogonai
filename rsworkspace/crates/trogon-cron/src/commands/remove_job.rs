use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};

use super::domain::JobId;

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecideError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for RemoveJobDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for RemoveJobDecideError {}

impl RemoveJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl Decider for RemoveJobCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = RemoveJobDecideError;
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
            return Err(RemoveJobDecideError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(RemoveJobDecideError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(RemoveJobDecideError::JobNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Ok(Decision::event(v1::JobEvent {
                    event: Some(v1::JobRemoved {}.into()),
                }))
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(RemoveJobDecideError::JobDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(RemoveJobDecideError::UnknownStateValue { value: 0 }),
        }
    }
}

impl CommandSnapshotPolicy for RemoveJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        CommandExecution, Events, run_task_immediately,
        testing::{TestCase, decider},
    };

    use super::*;
    use crate::commands::domain::{Delivery, Job, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule};
    use crate::{AddJobCommand, GetJobCommand, mocks::MockCronStore};

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

    fn removed() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        }
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
            .then_error(RemoveJobDecideError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn given_when_then_rejects_removing_deleted_job() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([added("backup")])
            .given([removed()])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then_error(RemoveJobDecideError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_removes_existing_job() {
        let store = MockCronStore::new();
        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();

        let outcome = CommandExecution::new(&store, &RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.stream_position.get(), 2);
        assert_eq!(outcome.events, Events::one(removed()));

        assert!(
            store
                .get_job(GetJobCommand::new(crate::JobId::parse("backup").unwrap()))
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
