use std::error::Error;

use buffa::{EnumValue, MessageField};
use trogon_decider::testing::TestCase;

use super::*;
use crate::commands::domain::{
    Delivery, MessageContent, Schedule as DomainSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
};

fn schedule_id(id: &str) -> ScheduleId {
    ScheduleId::parse(id).unwrap()
}

fn create_schedule(id: &str) -> CreateSchedule {
    CreateSchedule {
        id: schedule_id(id),
        status: ScheduleEventStatus::Scheduled,
        schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json(r#"{"kind":"heartbeat"}"#),
            headers: ScheduleHeaders::default(),
        },
    }
}

fn added(id: &str) -> v1::ScheduleEvent {
    let command = create_schedule(id);

    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: command.id.as_str().to_string(),
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

fn resumed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleResumed {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

#[test]
fn given_when_then_supports_create_schedule_decider() {
    TestCase::<CreateSchedule>::new()
        .given_no_history()
        .when(create_schedule("backup"))
        .then([added("backup")]);
}

#[test]
fn given_when_then_supports_create_schedule_failures() {
    TestCase::<CreateSchedule>::new()
        .given([added("backup")])
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::AlreadyExists {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn replaying_lifecycle_events_rejects_existing_schedule_ids() {
    TestCase::<CreateSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::AlreadyExists {
            id: ScheduleId::parse("backup").unwrap(),
        });

    TestCase::<CreateSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .given([resumed("backup")])
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::AlreadyExists {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn rejects_adding_deleted_schedule_ids() {
    TestCase::<CreateSchedule>::new()
        .given([added("backup")])
        .given([removed()])
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::ScheduleDeleted {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn decide_errors_display_user_facing_messages() {
    let id = ScheduleId::parse("backup").unwrap();
    let duration_error = trogonai_proto::convert::duration_from_std(
        trogonai_proto::convert::PROTOBUF_DURATION_MAX + std::time::Duration::from_nanos(1),
    )
    .unwrap_err();

    assert_eq!(
        CreateScheduleDecideError::AlreadyExists { id: id.clone() }.to_string(),
        "schedule 'backup' already exists"
    );
    assert_eq!(
        CreateScheduleDecideError::ScheduleDeleted { id: id.clone() }.to_string(),
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
    let err = CreateScheduleDecideError::DurationConversion { source: duration_error };
    assert_eq!(
        err.to_string(),
        "schedule duration is invalid: duration must fit the protobuf Duration range: max 315576000000s, got 315576000000.000000001s"
    );
    assert!(err.source().is_some());
    assert!(
        CreateScheduleDecideError::AlreadyExists { id: id.clone() }
            .source()
            .is_none()
    );
    assert!(CreateScheduleDecideError::ScheduleDeleted { id }.source().is_none());
    assert!(CreateScheduleDecideError::MissingStateValue.source().is_none());
    assert!(
        CreateScheduleDecideError::UnknownStateValue { value: 123 }
            .source()
            .is_none()
    );
}

#[test]
fn decide_rejects_invalid_state_values() {
    TestCase::<CreateSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: None,
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::MissingStateValue);

    TestCase::<CreateSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::UnknownStateValue { value: 123 });

    TestCase::<CreateSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(create_schedule("backup"))
        .then_error(CreateScheduleDecideError::UnknownStateValue { value: 0 });
}
