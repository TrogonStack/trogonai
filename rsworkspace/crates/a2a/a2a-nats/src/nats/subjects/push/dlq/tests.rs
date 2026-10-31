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

fn parse(subject: &str) -> Result<PushDlqSubject, PushDlqSubjectError> {
    PushDlqSubject::parse(&prefix(), subject)
}

#[test]
fn formats_the_dead_letter_under_the_callers_principal() {
    let s = PushDlqSubject::new(&prefix(), &caller(), &task());
    assert_eq!(s.to_string(), "a2a.v1.push.dlq.person.alice.task-1");
    assert_eq!(s.to_subject().as_str(), "a2a.v1.push.dlq.person.alice.task-1");
}

#[test]
fn principals_of_different_kinds_never_share_a_dead_letter_subject() {
    let person = PushDlqSubject::new(&prefix(), &caller(), &task());
    let agent = PushDlqSubject::new(
        &prefix(),
        &SubjectPrincipal::from_tokens("agent", "alice").unwrap(),
        &task(),
    );
    assert_ne!(person.to_string(), agent.to_string());
}

#[test]
fn parse_round_trips_the_caller_and_the_task() {
    let built = PushDlqSubject::new(&prefix(), &caller(), &task());
    let parsed = parse(&built.to_string()).unwrap();
    assert_eq!(parsed.caller(), &caller());
    assert_eq!(parsed.task_id(), &task());
    assert_eq!(parsed, built);
}

#[test]
fn parse_round_trips_under_a_multi_token_prefix_with_a_hashed_principal() {
    let prefix = A2aPrefix::new("my.multi.part").unwrap();
    let caller = SubjectPrincipal::from_principal(PrincipalKind::Service, &PrincipalId::new("svc.billing").unwrap());
    assert!(caller.id().is_hashed());
    let built = PushDlqSubject::new(&prefix, &caller, &task());
    assert_eq!(PushDlqSubject::parse(&prefix, &built.to_string()), Ok(built));
}

#[test]
fn mirror_keeps_the_principal_and_the_task() {
    let mirror = PushDlqSubject::new(&prefix(), &caller(), &task()).mirror();
    assert_eq!(mirror.to_string(), "a2a.v1.push.dlq.mirror.person.alice.task-1");
    assert_eq!(mirror.caller(), &caller());
    assert_eq!(mirror.task_id(), &task());
}

#[test]
fn parse_rejects_a_foreign_prefix_version_or_family() {
    for subject in [
        "other.v1.push.dlq.person.alice.task-1",
        "a2a.v2.push.dlq.person.alice.task-1",
        "a2a.v1.push.person.alice.task-1",
        "a2a.v1.tasks.task-1.person.alice.events",
        "a2a.v1.push.dlq",
    ] {
        assert_eq!(parse(subject), Err(PushDlqSubjectError::NotPushDlq), "{subject}");
    }
}

#[test]
fn parse_rejects_a_subject_without_the_principal_position() {
    for subject in [
        "a2a.v1.push.dlq.alice.task-1",
        "a2a.v1.push.dlq._.task-1",
        "a2a.v1.push.dlq.person.alice",
        "a2a.v1.push.dlq.person.alice.task-1.extra",
        "a2a.v1.push.dlq.mirror.person.alice.task-1",
        "a2a.v1.push.dlq.",
    ] {
        assert_eq!(parse(subject), Err(PushDlqSubjectError::BadSubjectShape), "{subject}");
    }
}

#[test]
fn parse_rejects_an_invalid_caller() {
    assert_eq!(
        parse("a2a.v1.push.dlq.mirror.alice.task-1"),
        Err(PushDlqSubjectError::InvalidCaller(SubjectPrincipalError::Kind(
            PrincipalKindError::Unknown
        )))
    );
    assert_eq!(
        parse("a2a.v1.push.dlq.person.*.task-1"),
        Err(PushDlqSubjectError::InvalidCaller(SubjectPrincipalError::Id(
            SubjectPrincipalIdError::InvalidCharacter('*')
        )))
    );
    assert_eq!(
        parse("a2a.v1.push.dlq.person..task-1"),
        Err(PushDlqSubjectError::InvalidCaller(SubjectPrincipalError::Id(
            SubjectPrincipalIdError::Empty
        )))
    );
}

#[test]
fn parse_rejects_an_invalid_task_id() {
    assert_eq!(
        parse("a2a.v1.push.dlq.person.alice.*"),
        Err(PushDlqSubjectError::InvalidTaskId(TaskIdError::InvalidCharacter('*')))
    );
}
