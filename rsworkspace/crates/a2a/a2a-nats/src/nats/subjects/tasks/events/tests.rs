use async_nats::subject::ToSubject;
use trogon_identity_types::{PrincipalId, PrincipalKind, PrincipalKindError, SubjectPrincipalIdError};

use super::*;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn task() -> A2aTaskId {
    A2aTaskId::new("task-1").unwrap()
}

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").unwrap()
}

fn parse(subject: &str) -> Result<TaskEventsSubject, TaskEventsSubjectError> {
    TaskEventsSubject::parse(&prefix(), subject)
}

#[test]
fn formats_prefix_tasks_events_subject_scoped_to_the_task_and_the_caller() {
    let s = TaskEventsSubject::new(&prefix(), &task(), &caller());
    assert_eq!(s.to_string(), "a2a.v1.tasks.task-1.person.alice.events");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TaskEventsSubject::new(&prefix(), &task(), &caller());
    assert_eq!(s.to_subject().as_str(), "a2a.v1.tasks.task-1.person.alice.events");
}

#[test]
fn subject_carries_no_request_id() {
    let s = TaskEventsSubject::new(&prefix(), &task(), &caller()).to_string();
    assert!(
        !s.split('.')
            .any(trogon_nats::subject_conformance::looks_like_request_id),
        "{s}"
    );
}

#[test]
fn parse_round_trips_the_task_and_the_caller() {
    let built = TaskEventsSubject::new(&prefix(), &task(), &caller());
    let parsed = parse(&built.to_string()).unwrap();
    assert_eq!(parsed.task_id(), &task());
    assert_eq!(parsed.caller(), &caller());
    assert_eq!(parsed, built);
}

#[test]
fn parse_round_trips_under_a_multi_token_prefix_with_a_hashed_principal() {
    let prefix = A2aPrefix::new("my.multi.part").unwrap();
    let caller = SubjectPrincipal::from_principal(PrincipalKind::Agent, &PrincipalId::new("agent.scout").unwrap());
    assert!(caller.id().is_hashed());
    let built = TaskEventsSubject::new(&prefix, &task(), &caller);
    assert_eq!(TaskEventsSubject::parse(&prefix, &built.to_string()), Ok(built));
}

#[test]
fn parse_rejects_a_foreign_prefix_version_or_family() {
    for subject in [
        "other.v1.tasks.task-1.person.alice.events",
        "a2a.v2.tasks.task-1.person.alice.events",
        "a2a.task.task-1.person.alice.events",
        "a2a.v1.gateway.person.alice.events",
        "a2a.v1.tasks",
    ] {
        assert_eq!(parse(subject), Err(TaskEventsSubjectError::NotTaskEvents), "{subject}");
    }
}

#[test]
fn parse_rejects_a_subject_without_the_principal_position_or_the_events_terminal() {
    for subject in [
        "a2a.v1.tasks.task-1.events",
        "a2a.v1.tasks.task-1.person.alice",
        "a2a.v1.tasks.task-1.person.alice.status",
        "a2a.v1.tasks.task-1.person.alice.events.r1",
        "a2a.v1.tasks.",
    ] {
        assert_eq!(
            parse(subject),
            Err(TaskEventsSubjectError::BadSubjectShape),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_invalid_task_id() {
    assert!(matches!(
        parse("a2a.v1.tasks.*.person.alice.events"),
        Err(TaskEventsSubjectError::InvalidTaskId(TaskIdError::InvalidCharacter(
            '*'
        )))
    ));
}

#[test]
fn parse_rejects_an_invalid_caller() {
    assert_eq!(
        parse("a2a.v1.tasks.task-1.robot.alice.events"),
        Err(TaskEventsSubjectError::InvalidCaller(SubjectPrincipalError::Kind(
            PrincipalKindError::Unknown
        )))
    );
    assert_eq!(
        parse("a2a.v1.tasks.task-1.person.*.events"),
        Err(TaskEventsSubjectError::InvalidCaller(SubjectPrincipalError::Id(
            SubjectPrincipalIdError::InvalidCharacter('*')
        )))
    );
    assert_eq!(
        parse("a2a.v1.tasks.task-1.person..events"),
        Err(TaskEventsSubjectError::InvalidCaller(SubjectPrincipalError::Id(
            SubjectPrincipalIdError::Empty
        )))
    );
}
