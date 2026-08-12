use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_prefix_tasks_events_subject_scoped_to_the_task() {
    let s = TaskEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-1").unwrap());
    assert_eq!(s.to_string(), "a2a.v1.tasks.task-1.events");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TaskEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-1").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.v1.tasks.task-1.events");
}

#[test]
fn subject_carries_no_request_id() {
    let s = TaskEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-1").unwrap()).to_string();
    assert!(
        !s.split('.')
            .any(trogon_nats::subject_conformance::looks_like_request_id),
        "{s}"
    );
}
