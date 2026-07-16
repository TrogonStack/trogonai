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

fn resume_job_command(id: &str) -> ResumeSchedule {
    ResumeSchedule::new(ScheduleId::parse(id).unwrap())
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

fn completed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCompleted {
                schedule_id: id.to_string(),
                last_occurrence_sequence: Some(0),
            }
            .into(),
        ),
    }
}

#[test]
fn given_when_then_supports_resume_job_decider() {
    TestCase::<ResumeSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .when(resume_job_command("backup"))
        .then([resumed("backup")]);
}

#[test]
fn given_when_then_supports_resume_job_failures() {
    TestCase::<ResumeSchedule>::new()
        .given([added("backup")])
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::AlreadyActive {
            id: ScheduleId::parse("backup").unwrap(),
        })
        .then_error_code("already-active");
}

#[test]
fn given_when_then_rejects_resuming_completed_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given([added("backup")])
        .given([completed("backup")])
        .given([paused("backup")])
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::AlreadyCompleted {
            id: ScheduleId::parse("backup").unwrap(),
        })
        .then_error_code("already-completed");
}

#[test]
fn given_when_then_rejects_resuming_missing_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given_no_history()
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::ScheduleNotFound {
            id: ScheduleId::parse("backup").unwrap(),
        })
        .then_error_code("schedule-not-found");
}

#[test]
fn given_when_then_rejects_resuming_deleted_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given([added("backup")])
        .given([paused("backup")])
        .given([removed("backup")])
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::ScheduleDeleted {
            id: ScheduleId::parse("backup").unwrap(),
        })
        .then_error_code("schedule-deleted");
}

#[test]
fn errors_display_user_facing_messages() {
    let id = ScheduleId::parse("backup").unwrap();

    assert_eq!(
        ResumeScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
        "schedule 'backup' does not exist"
    );
    assert_eq!(
        ResumeScheduleError::ScheduleDeleted { id: id.clone() }.to_string(),
        "schedule 'backup' was deleted"
    );
    assert_eq!(
        ResumeScheduleError::AlreadyActive { id: id.clone() }.to_string(),
        "schedule 'backup' is already active"
    );
    assert_eq!(
        ResumeScheduleError::AlreadyCompleted { id }.to_string(),
        "schedule 'backup' has already completed its recurrence"
    );
    assert_eq!(
        ResumeScheduleError::MissingStateValue.to_string(),
        "state value is missing"
    );
    assert_eq!(
        ResumeScheduleError::UnknownStateValue { value: 42 }.to_string(),
        "unknown state value: 42"
    );
}

#[test]
fn decide_rejects_invalid_state_values() {
    TestCase::<ResumeSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: None,
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::MissingStateValue)
        .then_error_code("missing-state-value");

    TestCase::<ResumeSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::UnknownStateValue { value: 123 })
        .then_error_code("unknown-state-value");

    TestCase::<ResumeSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(resume_job_command("backup"))
        .then_error(ResumeScheduleError::UnknownStateValue { value: 0 })
        .then_error_code("unknown-state-value");
}
