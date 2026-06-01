use buffa::EnumValue;
use trogon_decider_runtime::Decider;
use trogon_scheduler::commands::domain::{
    Delivery, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule, ScheduleId,
};
use trogon_scheduler::{CreateScheduleCommand, CreateScheduleDecideError};
use trogonai_proto::scheduler::schedules::state_v1;

fn command() -> CreateScheduleCommand {
    CreateScheduleCommand {
        id: ScheduleId::parse("backup").unwrap(),
        status: JobStatus::Enabled,
        schedule: Schedule::every(30).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: JobMessage {
            content: MessageContent::json("{}"),
            headers: JobHeaders::default(),
        },
    }
}

#[test]
fn create_schedule_decide_errors_display_user_facing_messages() {
    let id = ScheduleId::parse("backup").unwrap();

    assert_eq!(
        CreateScheduleDecideError::AlreadyExists { id: id.clone() }.to_string(),
        "schedule 'backup' already exists"
    );
    assert_eq!(
        CreateScheduleDecideError::JobDeleted { id }.to_string(),
        "schedule 'backup' was deleted"
    );
    assert_eq!(
        CreateScheduleDecideError::MissingStateValue.to_string(),
        "state value is missing"
    );
    assert_eq!(
        CreateScheduleDecideError::UnknownStateValue { value: 123 }.to_string(),
        "unknown state value: 123"
    );
}

#[test]
fn create_schedule_decide_rejects_invalid_state_values() {
    assert_eq!(
        CreateScheduleCommand::decide(&state_v1::State { state: None }, &command()).unwrap_err(),
        CreateScheduleDecideError::MissingStateValue
    );
    assert_eq!(
        CreateScheduleCommand::decide(
            &state_v1::State {
                state: Some(EnumValue::from(123)),
            },
            &command()
        )
        .unwrap_err(),
        CreateScheduleDecideError::UnknownStateValue { value: 123 }
    );
    assert_eq!(
        CreateScheduleCommand::decide(
            &state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            },
            &command()
        )
        .unwrap_err(),
        CreateScheduleDecideError::UnknownStateValue { value: 0 }
    );
}
