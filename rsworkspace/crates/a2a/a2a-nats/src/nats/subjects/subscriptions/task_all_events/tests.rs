use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_global_tasks_events_filter() {
    let s = TaskAllEventsSubject::new(&A2aPrefix::new("a2a").unwrap());
    assert_eq!(s.to_string(), "a2a.tasks.*.events.*");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TaskAllEventsSubject::new(&A2aPrefix::new("a2a").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.tasks.*.events.*");
}
