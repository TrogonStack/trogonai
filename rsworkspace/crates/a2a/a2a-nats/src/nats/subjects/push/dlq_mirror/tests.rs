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

fn parse(subject: &str) -> Result<PushDlqMirrorSubject, PushDlqMirrorSubjectError> {
    PushDlqMirrorSubject::parse(&prefix(), subject)
}

#[test]
fn formats_the_mirror_under_the_callers_principal() {
    let s = PushDlqMirrorSubject::new(&prefix(), &caller(), &task());
    assert_eq!(s.to_string(), "a2a.v1.push.dlq.mirror.person.alice.task-1");
    assert_eq!(s.to_subject().as_str(), "a2a.v1.push.dlq.mirror.person.alice.task-1");
}

#[test]
fn parse_round_trips_the_caller_and_the_task() {
    let built = PushDlqMirrorSubject::new(&prefix(), &caller(), &task());
    let parsed = parse(&built.to_string()).unwrap();
    assert_eq!(parsed.caller(), &caller());
    assert_eq!(parsed.task_id(), &task());
    assert_eq!(parsed, built);
}

#[test]
fn parse_round_trips_under_a_multi_token_prefix_with_a_hashed_principal() {
    let prefix = A2aPrefix::new("my.multi.part").unwrap();
    let caller = SubjectPrincipal::from_principal(PrincipalKind::Agent, &PrincipalId::new("agent.scout").unwrap());
    assert!(caller.id().is_hashed());
    let built = PushDlqMirrorSubject::new(&prefix, &caller, &task());
    assert_eq!(PushDlqMirrorSubject::parse(&prefix, &built.to_string()), Ok(built));
}

#[test]
fn parse_rejects_anything_outside_the_mirror_lane() {
    for subject in [
        "a2a.v1.push.dlq.person.alice.task-1",
        "a2a.v1.push.dlq.person.mirror.task-1",
        "other.v1.push.dlq.mirror.person.alice.task-1",
        "a2a.v2.push.dlq.mirror.person.alice.task-1",
        "a2a.v1.push.dlq.mirror",
    ] {
        assert_eq!(
            parse(subject),
            Err(PushDlqMirrorSubjectError::NotPushDlqMirror),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_a_mirror_without_the_principal_position() {
    for subject in [
        "a2a.v1.push.dlq.mirror.alice.task-1",
        "a2a.v1.push.dlq.mirror.person.alice",
        "a2a.v1.push.dlq.mirror.person.alice.task-1.extra",
        "a2a.v1.push.dlq.mirror.",
    ] {
        assert_eq!(
            parse(subject),
            Err(PushDlqMirrorSubjectError::BadSubjectShape),
            "{subject}"
        );
    }
}

#[test]
fn parse_rejects_an_invalid_caller_or_task_id() {
    assert_eq!(
        parse("a2a.v1.push.dlq.mirror.robot.alice.task-1"),
        Err(PushDlqMirrorSubjectError::InvalidCaller(SubjectPrincipalError::Kind(
            PrincipalKindError::Unknown
        )))
    );
    assert_eq!(
        parse("a2a.v1.push.dlq.mirror.person.*.task-1"),
        Err(PushDlqMirrorSubjectError::InvalidCaller(SubjectPrincipalError::Id(
            SubjectPrincipalIdError::InvalidCharacter('*')
        )))
    );
    assert_eq!(
        parse("a2a.v1.push.dlq.mirror.person.alice.>"),
        Err(PushDlqMirrorSubjectError::InvalidTaskId(TaskIdError::InvalidCharacter(
            '>'
        )))
    );
}
