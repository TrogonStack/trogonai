use super::*;
use async_nats::subject::ToSubject;

#[test]
fn formats_prefix_agent_tasks_get_subject() {
    let s = TasksGetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.tasks.get");
}

#[test]
fn to_subject_round_trips_display_form() {
    let s = TasksGetSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.tasks.get");
}
