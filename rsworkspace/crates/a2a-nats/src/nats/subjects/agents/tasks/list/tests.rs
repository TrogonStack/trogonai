use super::*;

#[test]
fn formats_prefix_agent_tasks_list_subject() {
    let s = TasksListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.tasks.list");
}

#[test]
fn to_subject_round_trips_display_form() {
    use async_nats::subject::ToSubject;
    let s = TasksListSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.tasks.list");
}
