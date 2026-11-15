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
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([paused("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then([resumed("0198fa2f6d0a7b1a8cf9f762e73a1c45")]);
}

#[test]
fn given_when_then_supports_resume_job_failures() {
    TestCase::<ResumeSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::AlreadyActive {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_resuming_completed_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([completed("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([paused("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::AlreadyCompleted {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_resuming_missing_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given_no_history()
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::ScheduleNotFound {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn given_when_then_rejects_resuming_deleted_jobs() {
    TestCase::<ResumeSchedule>::new()
        .given([added("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([paused("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .given([removed("0198fa2f6d0a7b1a8cf9f762e73a1c45")])
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::ScheduleDeleted {
            id: ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap(),
        });
}

#[test]
fn errors_display_user_facing_messages() {
    let id = ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c45").unwrap();

    assert_eq!(
        ResumeScheduleError::ScheduleNotFound { id: id.clone() }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' does not exist"
    );
    assert_eq!(
        ResumeScheduleError::ScheduleDeleted { id: id.clone() }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' was deleted"
    );
    assert_eq!(
        ResumeScheduleError::AlreadyActive { id: id.clone() }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' is already active"
    );
    assert_eq!(
        ResumeScheduleError::AlreadyCompleted { id }.to_string(),
        "schedule '0198fa2f6d0a7b1a8cf9f762e73a1c45' has already completed its recurrence"
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
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::MissingStateValue);

    TestCase::<ResumeSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::UnknownStateValue { value: 123 });

    TestCase::<ResumeSchedule>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(resume_job_command("0198fa2f6d0a7b1a8cf9f762e73a1c45"))
        .then_error(ResumeScheduleError::UnknownStateValue { value: 0 });
}
