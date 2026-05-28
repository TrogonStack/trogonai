use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{Job, ScheduleId, schedule_created_from_job};

#[derive(Debug, Clone)]
pub struct AddScheduleCommand {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddScheduleDecideError {
    AlreadyExists { id: ScheduleId },
    JobDeleted { id: ScheduleId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for AddScheduleDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for AddScheduleDecideError {}

impl AddScheduleCommand {
    pub fn new(job: Job) -> Self {
        Self { job }
    }
}

impl Decider for AddScheduleCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = AddScheduleDecideError;
    type EvolveError = super::EvolveError;

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        self.job.id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(value) = state.state.as_ref() else {
            return Err(AddScheduleDecideError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(AddScheduleDecideError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => Ok(Decision::event(v1::ScheduleEvent {
                event: Some(schedule_created_from_job(&command.job).into()),
            })),
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(AddScheduleDecideError::AlreadyExists {
                    id: command.job.id.clone(),
                })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => Err(AddScheduleDecideError::JobDeleted {
                id: command.job.id.clone(),
            }),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(AddScheduleDecideError::UnknownStateValue { value: 0 })
            }
        }
    }
}

impl CommandSnapshotPolicy for AddScheduleCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_decider::testing::TestCase;
    use trogon_decider_runtime::{CommandError, CommandExecution, Events, ImmediateSnapshotTaskScheduler};

    use super::*;
    use crate::commands::domain::{
        Delivery, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule as DomainSchedule,
    };
    use crate::{
        GetScheduleCommand, MessageContent as ReadMessageContent, MessageEnvelope, MessageHeaders, Schedule,
        ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus, mocks::MockSchedulerStore,
    };

    fn job_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn add_job_command(id: &str) -> AddScheduleCommand {
        AddScheduleCommand::new(job(id))
    }

    fn remove_job_command(id: &str) -> crate::RemoveScheduleCommand {
        crate::RemoveScheduleCommand::new(ScheduleId::parse(id).unwrap())
    }

    fn job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: DomainSchedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            status: ScheduleEventStatus::Scheduled,
            schedule: ScheduleEventSchedule::Every { every_sec: 30 },
            delivery: ScheduleEventDelivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl_sec: None,
                source: None,
            },
            message: MessageEnvelope {
                content: ReadMessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(schedule_created_from_job(&job(id)).into()),
        }
    }

    fn removed() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::<AddScheduleCommand>::new()
            .given_no_history()
            .when(add_job_command("backup"))
            .then([added("backup")]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::<AddScheduleCommand>::new()
            .given([added("backup")])
            .when(add_job_command("backup"))
            .then_error(AddScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::<AddScheduleCommand>::new()
            .given([added("backup")])
            .given([removed()])
            .when(add_job_command("backup"))
            .then_error(AddScheduleDecideError::JobDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockSchedulerStore::new();

        let outcome = CommandExecution::new(&store, &add_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.stream_position.as_u64(), 1);
        assert_eq!(outcome.events, Events::one(added("backup")));

        let stored_job = store
            .get_schedule(GetScheduleCommand::new(crate::ScheduleId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<state_v1::State>(&ScheduleId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockSchedulerStore::new();

        CommandExecution::new(&store, &add_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &add_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(AddScheduleDecideError::AlreadyExists { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_adding_deleted_job_id() {
        let store = MockSchedulerStore::new();

        CommandExecution::new(&store, &add_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap();
        CommandExecution::new(&store, &remove_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &add_job_command("backup"))
            .with_snapshot(&store)
            .with_task_runtime(ImmediateSnapshotTaskScheduler)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandError::Decide(AddScheduleDecideError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
