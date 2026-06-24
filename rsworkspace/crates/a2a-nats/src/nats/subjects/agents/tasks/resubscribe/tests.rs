use super::*;

#[test]
fn formats_prefix_agent_tasks_resubscribe_subject() {
    let s = TasksResubscribeSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_string(), "a2a.agents.planner.tasks.resubscribe");
}

#[test]
fn to_subject_round_trips_display_form() {
    use async_nats::subject::ToSubject;
    let s = TasksResubscribeSubject::new(&A2aPrefix::new("a2a").unwrap(), &A2aAgentId::new("planner").unwrap());
    assert_eq!(s.to_subject().as_str(), "a2a.agents.planner.tasks.resubscribe");
}
