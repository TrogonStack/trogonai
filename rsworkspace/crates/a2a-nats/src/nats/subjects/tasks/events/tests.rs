use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_prefix_tasks_events_subject_with_req_id_suffix() {
    let s = TaskEventsSubject::new(
        &A2aPrefix::new("a2a").unwrap(),
        &A2aTaskId::new("task-1").unwrap(),
        &ReqId::from_test("r1"),
    );
    assert_eq!(s.to_string(), "a2a.tasks.task-1.events.r1");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TaskEventsSubject::new(
        &A2aPrefix::new("a2a").unwrap(),
        &A2aTaskId::new("task-1").unwrap(),
        &ReqId::from_test("r1"),
    );
    assert_eq!(s.to_subject().as_str(), "a2a.tasks.task-1.events.r1");
}
