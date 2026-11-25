use async_nats::subject::ToSubject;
use trogon_identity_types::{PrincipalId, PrincipalKind};

use super::*;

#[test]
fn formats_every_task_of_one_caller() {
    let caller = SubjectPrincipal::from_tokens("person", "alice").unwrap();
    let s = CallerTaskEventsSubject::new(&A2aPrefix::new("a2a").unwrap(), &caller);
    assert_eq!(s.to_string(), "a2a.v1.tasks.*.person.alice.events");
    assert_eq!(s.to_subject().as_str(), "a2a.v1.tasks.*.person.alice.events");
}

#[test]
fn carries_a_hashed_principal_as_one_token() {
    let caller =
        SubjectPrincipal::from_principal(PrincipalKind::Person, &PrincipalId::new("alice@example.com").unwrap());
    let s = CallerTaskEventsSubject::new(&A2aPrefix::new("my.app").unwrap(), &caller).to_string();
    assert_eq!(s, format!("my.app.v1.tasks.*.person.{}.events", caller.id()));
    assert_eq!(s.split('.').count(), 8);
}
