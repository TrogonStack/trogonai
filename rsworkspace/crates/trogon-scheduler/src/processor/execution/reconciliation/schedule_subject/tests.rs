use super::*;
use crate::commands::domain::ScheduleId;

fn key(raw: &str) -> ScheduleKey {
    ScheduleKey::derive(&ScheduleId::parse(raw).unwrap())
}

#[test]
fn execution_subject_uses_the_execution_prefix_and_key() {
    let key = key("orders/created");
    let subject = ScheduleSubject::execution(&key);

    assert_eq!(
        subject.as_str(),
        format!("scheduler.schedules.execution.v1.{}", key.simple())
    );
}

#[test]
fn event_subject_uses_the_event_prefix_and_key() {
    let key = key("orders/created");
    let subject = ScheduleSubject::event(&key);

    assert_eq!(
        subject.as_str(),
        format!("scheduler.schedules.events.v1.{}", key.simple())
    );
}

#[test]
fn rrule_wakeup_subject_uses_the_rrule_execution_prefix_and_key() {
    let key = key("orders/created");
    let subject = ScheduleSubject::rrule_wakeup(&key);

    assert_eq!(
        subject.as_str(),
        format!("scheduler.schedules.execution.v1.rrule.{}", key.simple())
    );
}

#[test]
fn subjects_are_deterministic_for_a_given_key() {
    let key = key("ns:thing");

    assert_eq!(ScheduleSubject::execution(&key), ScheduleSubject::execution(&key));
}

#[test]
fn display_matches_as_str() {
    let key = key("orders/created");

    for subject in [
        ScheduleSubject::execution(&key),
        ScheduleSubject::event(&key),
        ScheduleSubject::rrule_wakeup(&key),
    ] {
        assert_eq!(subject.to_string(), subject.as_str());
    }
}

#[test]
fn is_scheduler_internal_matches_execution_event_and_internal_namespaces() {
    assert!(ScheduleSubject::is_scheduler_internal("scheduler.schedules.execution.v1"));
    assert!(ScheduleSubject::is_scheduler_internal(
        "scheduler.schedules.execution.v1.orders.created"
    ));
    assert!(ScheduleSubject::is_scheduler_internal("scheduler.schedules.events.v1.orders.created"));
    assert!(ScheduleSubject::is_scheduler_internal("trogon.scheduler"));
    assert!(ScheduleSubject::is_scheduler_internal("trogon.scheduler.sentinel"));
}

#[test]
fn is_scheduler_internal_rejects_unrelated_subjects() {
    assert!(!ScheduleSubject::is_scheduler_internal("agent.run"));
    assert!(!ScheduleSubject::is_scheduler_internal("scheduler.other.v1"));
    assert!(!ScheduleSubject::is_scheduler_internal("trogon.other"));
}
