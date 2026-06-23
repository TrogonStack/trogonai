use buffa::{EnumValue, MessageField};
use trogon_decider::testing::TestCase;

use super::*;
use crate::commands::domain::{
    Delivery, MessageContent, MessageEnvelope, Schedule as DomainSchedule, ScheduleEventDelivery,
    ScheduleEventSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
};

fn schedule_id(id: &str) -> ScheduleId {
    ScheduleId::parse(id).unwrap()
}

fn pause_job_command(id: &str) -> PauseSchedule {
    PauseSchedule::new(ScheduleId::parse(id).unwrap())
}

fn create_schedule(id: &str, status: ScheduleEventStatus) -> crate::CreateSchedule {
    crate::CreateSchedule {
        id: schedule_id(id),
        status,
        schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
            headers: ScheduleHeaders::default(),
        },
    }
}

fn added(id: &str) -> v1::ScheduleEvent {
    let command = create_schedule(id, ScheduleEventStatus::Scheduled);

    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: command.id.as_str().to_string(),
                status: buffa::MessageField::some(v1::ScheduleStatus::from(command.status)),
                schedule: buffa::MessageField::some(
                    v1::Schedule::try_from(&ScheduleEventSchedule::from(&command.schedule)).unwrap(),
                ),
                delivery: buffa::MessageField::some(
                    v1::Delivery::try_from(&ScheduleEventDelivery::from(&command.delivery)).unwrap(),
                ),
                message: buffa::MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
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

#[test]
fn given_when_then_supports_pause_job_decider() {
    TestCase::<PauseSchedule>::new()
        .given([added("backup")])
        .when(pause_job_command("backup"))
        .then([paused("backup")]);
}

#[test]
fn given_when_then_supports_pause_job_failures() {
    TestCase::<PauseSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::AlreadyPaused {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_pausing_missing_jobs() {
    TestCase::<PauseSchedule>::new()
        .given_no_history()
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::ScheduleNotFound {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_pausing_deleted_jobs() {
    TestCase::<PauseSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .given([removed("backup")])
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::ScheduleDeleted {
            id: ScheduleId::parse("backup").unwrap(),
        });
}

#[test]
fn decide_errors_display_user_facing_messages() {
    let id = ScheduleId::parse("backup").unwrap();

    assert_eq!(
        PauseScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
        "schedule 'backup' does not exist"
    );
    assert_eq!(
        PauseScheduleError::ScheduleDeleted { id: id.clone() }.to_string(),
        "schedule 'backup' was deleted"
    );
    assert_eq!(
        PauseScheduleError::AlreadyPaused { id }.to_string(),
        "schedule 'backup' is already paused"
    );
    assert_eq!(
        PauseScheduleError::MissingStateValue.to_string(),
        "state value is missing"
    );
    assert_eq!(
        PauseScheduleError::UnknownStateValue { value: 42 }.to_string(),
        "unknown state value: 42"
    );
}

#[test]
fn decide_rejects_invalid_state_values() {
    TestCase::<PauseSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: None,
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::MissingStateValue);

    TestCase::<PauseSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::UnknownStateValue { value: 123 });

    TestCase::<PauseSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(pause_job_command("backup"))
        .then_error(PauseScheduleError::UnknownStateValue { value: 0 });
}

#[test]
fn command_snapshot_policy_uses_shared_frequency() {
    assert_eq!(
        <PauseSchedule as CommandSnapshotPolicy>::SNAPSHOT_POLICY
            .frequency()
            .get(),
        32
    );
}
