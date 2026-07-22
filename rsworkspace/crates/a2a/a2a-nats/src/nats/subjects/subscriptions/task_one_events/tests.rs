use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_per_task_events_filter() {
    let s = TaskOneEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-7").unwrap());
    assert_eq!(s.to_string(), "a2a.tasks.task-7.events.*");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TaskOneEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aTaskId::new("task-7").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.tasks.task-7.events.*");
}
