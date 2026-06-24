use super::*;

#[test]
fn formats_prefix_agent_push_get_subject() {
    let s = PushGetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.push.get");
}

#[test]
fn to_subject_round_trips_display_form() {
    use async_nats::subject::ToSubject;
    let s = PushGetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.push.get");
}
