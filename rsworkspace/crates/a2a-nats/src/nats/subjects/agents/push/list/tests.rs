use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_prefix_agent_push_list_subject() {
    let s = PushListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.push.list");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = PushListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.push.list");
}
