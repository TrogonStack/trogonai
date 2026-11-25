use super::*;
use async_nats::subject::ToSubject;

fn caller() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "alice").unwrap()
}

#[test]
fn formats_prefix_agent_push_set_subject() {
    let s = PushSetSubject::new(
        &A2aPrefix::new("a2a").unwrap(),
        &A2aAgentId::new("planner").unwrap(),
        &caller(),
    );
    assert_eq!(s.to_string(), "a2a.v1.agents.planner.person.alice.push.set");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = PushSetSubject::new(
        &A2aPrefix::new("a2a").unwrap(),
        &A2aAgentId::new("planner").unwrap(),
        &caller(),
    );
    assert_eq!(s.to_subject().as_str(), "a2a.v1.agents.planner.person.alice.push.set");
}
