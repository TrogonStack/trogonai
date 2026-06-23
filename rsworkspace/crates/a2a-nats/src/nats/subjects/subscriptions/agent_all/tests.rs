use super::*;

#[test]
fn formats_agent_all_subject_with_wildcard_tail() {
    let s = AgentAllSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.>");
}

#[test]
fn to_subject_round_trips_display_form() {
    use async_nats::subject::ToSubject;
    let s = AgentAllSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.>");
}
