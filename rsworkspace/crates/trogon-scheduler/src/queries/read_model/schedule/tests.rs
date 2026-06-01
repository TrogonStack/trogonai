use super::*;
use crate::queries::read_model::{
    MessageContent, MessageEnvelope, MessageHeaders, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
};

fn details(status: ScheduleEventStatus) -> ScheduleDetails {
    ScheduleDetails {
        status,
        schedule: ScheduleEventSchedule::Every {
            every: std::time::Duration::from_secs(30),
        },
        delivery: ScheduleEventDelivery::NatsMessage {
            subject: "agent.run".to_string(),
            ttl: None,
            source: None,
        },
        message: MessageEnvelope {
            content: MessageContent::from_static("application/json", "{}"),
            headers: MessageHeaders::default(),
        },
    }
}

#[test]
fn from_details_defaults_occurrences_and_completed() {
    let schedule = Schedule::from(("alpha".to_string(), details(ScheduleEventStatus::Scheduled)));

    assert_eq!(schedule.id, "alpha");
    assert!(!schedule.completed);
    assert!(schedule.next_occurrence_at.is_none());
    assert!(schedule.last_occurrence_at.is_none());
    assert!(schedule.is_enabled());
}

#[test]
fn is_enabled_is_false_when_paused() {
    let paused = Schedule::from(("a".to_string(), details(ScheduleEventStatus::Paused)));
    assert!(!paused.is_enabled());
}

#[test]
fn is_enabled_is_false_when_completed() {
    let mut completed = Schedule::from(("b".to_string(), details(ScheduleEventStatus::Scheduled)));
    completed.completed = true;
    assert!(!completed.is_enabled());
}

#[test]
fn serde_round_trips_through_json() {
    let schedule = Schedule::from(("orders/created".to_string(), details(ScheduleEventStatus::Scheduled)));
    let json = serde_json::to_string(&schedule).unwrap();
    let decoded: Schedule = serde_json::from_str(&json).unwrap();
    assert_eq!(schedule, decoded);
}
