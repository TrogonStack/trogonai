use buffa::{EnumValue, MessageField};
use trogon_decider::testing::TestCase;

use super::*;
use crate::CreateSchedule;
use crate::commands::domain::{
    Delivery, MessageContent, MessageEnvelope, Schedule as DomainSchedule, ScheduleEventDelivery,
    ScheduleEventSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleId, ScheduleMessage,
};

fn create_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: ScheduleId::parse(id).unwrap(),
        status: ScheduleEventStatus::Scheduled,
        schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: ScheduleHeaders::default(),
        },
    }
}

fn remove_job_command(id: &str) -> RemoveSchedule {
    RemoveSchedule::new(ScheduleId::parse(id).unwrap())
}

fn added(id: &str) -> v1::ScheduleEvent {
    let command = create_schedule(id);

    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: command.id.to_string(),
                status: MessageField::some(v1::ScheduleStatus::from(command.status)),
                schedule: MessageField::some(
                    v1::Schedule::try_from(&ScheduleEventSchedule::from(&command.schedule)).unwrap(),
                ),
                delivery: MessageField::some(
                    v1::Delivery::try_from(&ScheduleEventDelivery::from(&command.delivery)).unwrap(),
                ),
                message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
            }
            .into(),
        ),
    }
}

fn removed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleRemoved {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

fn paused(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::SchedulePaused {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

#[test]
fn given_when_then_supports_remove_job_decider() {
    TestCase::<RemoveSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then([removed("0198fa2f6d0a7b1a8cf9f762e73a1c45")]);
}

#[test]
fn given_when_then_removes_disabled_jobs() {
    TestCase::<RemoveSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([paused("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then([removed("0198fa2f6d0a7b1a8cf9f762e73a1c45")]);
}

#[test]
fn given_when_then_rejects_removing_missing_jobs() {
    TestCase::<RemoveSchedule>::new()
        .given_no_history()
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(RemoveScheduleError::ScheduleNotFound {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_removing_deleted_job() {
    TestCase::<RemoveSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([removed("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(RemoveScheduleError::ScheduleDeleted {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn errors_display_user_facing_messages() {
    let id = ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap();

    assert_eq!(
        RemoveScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' does not exist"
    );
    assert_eq!(
        RemoveScheduleError::ScheduleDeleted { id }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' was deleted"
    );
    assert_eq!(
        RemoveScheduleError::MissingStateValue.to_string(),
        "state value is missing"
    );
    assert_eq!(
        RemoveScheduleError::UnknownStateValue { value: 42 }.to_string(),
        "unknown state value: 42"
    );
}

#[test]
fn decide_rejects_invalid_state_values() {
    TestCase::<RemoveSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: None,
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(RemoveScheduleError::MissingStateValue);

    TestCase::<RemoveSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(RemoveScheduleError::UnknownStateValue { value: 123 });

    TestCase::<RemoveSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(RemoveScheduleError::UnknownStateValue { value: 0 });
}

#[test]
fn decider_trait_methods_delegate_to_schedule_state() {
    let command = remove_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45");

    assert_eq!(command.stream_id(), &command.id);
    assert_eq!(
        RemoveSchedule::initial_state().state.unwrap().as_known(),
        Some(state_v1::StateValue::STATE_VALUE_MISSING)
    );
}
