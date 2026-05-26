pub mod agent;
pub mod markers;
pub mod stream;
pub mod subscriptions;
pub mod task;

pub use stream::{A2aStream, StreamAssignment};

pub mod wildcards {
    pub use super::subscriptions::{AgentAllSubject, TaskAllEventsSubject, TaskOneEventsSubject};
}

#[cfg(test)]
mod tests {
    use super::{A2aStream, StreamAssignment, agent, task, wildcards};
    use crate::a2a_prefix::A2aPrefix;
    use crate::agent_id::A2aAgentId;
    use crate::req_id::ReqId;
    use crate::task_id::A2aTaskId;

    fn p(s: &str) -> A2aPrefix {
        A2aPrefix::new(s.to_string()).expect("test prefix")
    }

    fn aid(s: &str) -> A2aAgentId {
        A2aAgentId::new(s).expect("test agent id")
    }

    fn tid(s: &str) -> A2aTaskId {
        A2aTaskId::new(s).expect("test task id")
    }

    fn rid(s: &str) -> ReqId {
        ReqId::from_test(s)
    }

    #[test]
    fn agent_message_send() {
        assert_eq!(
            agent::MessageSendSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.message.send"
        );
    }

    #[test]
    fn agent_message_stream() {
        assert_eq!(
            agent::MessageStreamSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.message.stream"
        );
    }

    #[test]
    fn agent_tasks_get() {
        assert_eq!(
            agent::TasksGetSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.get"
        );
    }

    #[test]
    fn agent_tasks_list() {
        assert_eq!(
            agent::TasksListSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.list"
        );
    }

    #[test]
    fn agent_tasks_cancel() {
        assert_eq!(
            agent::TasksCancelSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.cancel"
        );
    }

    #[test]
    fn agent_tasks_resubscribe() {
        assert_eq!(
            agent::TasksResubscribeSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.resubscribe"
        );
    }

    #[test]
    fn agent_push_set() {
        assert_eq!(
            agent::PushSetSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.push_notification_config.set"
        );
    }

    #[test]
    fn agent_push_get() {
        assert_eq!(
            agent::PushGetSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.push_notification_config.get"
        );
    }

    #[test]
    fn agent_push_list() {
        assert_eq!(
            agent::PushListSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.push_notification_config.list"
        );
    }

    #[test]
    fn agent_push_delete() {
        assert_eq!(
            agent::PushDeleteSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.tasks.push_notification_config.delete"
        );
    }

    #[test]
    fn agent_card() {
        assert_eq!(
            agent::AgentCardSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.agent.card"
        );
    }

    #[test]
    fn task_events_full_subject() {
        assert_eq!(
            task::TaskEventsSubject::new(&p("a2a"), &tid("t1"), &rid("r1")).to_string(),
            "a2a.task.t1.events.r1"
        );
    }

    #[test]
    fn wildcard_agent_all() {
        assert_eq!(
            wildcards::AgentAllSubject::new(&p("a2a"), &aid("planner")).to_string(),
            "a2a.agent.planner.>"
        );
    }

    #[test]
    fn wildcard_task_one_events() {
        assert_eq!(
            wildcards::TaskOneEventsSubject::new(&p("a2a"), &tid("t1")).to_string(),
            "a2a.task.t1.events.*"
        );
    }

    #[test]
    fn wildcard_task_all_events() {
        assert_eq!(
            wildcards::TaskAllEventsSubject::new(&p("a2a")).to_string(),
            "a2a.task.*.events.*"
        );
    }

    #[test]
    fn custom_prefix() {
        assert_eq!(
            agent::MessageSendSubject::new(&p("myapp"), &aid("planner")).to_string(),
            "myapp.agent.planner.message.send"
        );
        assert_eq!(
            task::TaskEventsSubject::new(&p("myapp"), &tid("t1"), &rid("r1")).to_string(),
            "myapp.task.t1.events.r1"
        );
    }

    #[test]
    fn stream_assignments() {
        assert_eq!(agent::MessageSendSubject::STREAM, None);
        assert_eq!(agent::MessageStreamSubject::STREAM, None);
        assert_eq!(agent::TasksGetSubject::STREAM, None);
        assert_eq!(agent::TasksListSubject::STREAM, None);
        assert_eq!(agent::TasksCancelSubject::STREAM, None);
        assert_eq!(agent::TasksResubscribeSubject::STREAM, None);
        assert_eq!(agent::PushSetSubject::STREAM, None);
        assert_eq!(agent::PushGetSubject::STREAM, None);
        assert_eq!(agent::PushListSubject::STREAM, None);
        assert_eq!(agent::PushDeleteSubject::STREAM, None);
        assert_eq!(agent::AgentCardSubject::STREAM, None);
        assert_eq!(task::TaskEventsSubject::STREAM, Some(A2aStream::Events));
        assert_eq!(wildcards::AgentAllSubject::STREAM, None);
        assert_eq!(wildcards::TaskOneEventsSubject::STREAM, None);
        assert_eq!(wildcards::TaskAllEventsSubject::STREAM, None);
    }

    #[test]
    fn a2a_stream_display() {
        assert_eq!(A2aStream::Events.to_string(), "EVENTS");
        assert_eq!(A2aStream::PushDlq.to_string(), "PUSH_DLQ");
    }

    #[test]
    fn a2a_stream_name_uses_prefix() {
        assert_eq!(A2aStream::Events.stream_name(&p("a2a")), "A2A_EVENTS");
        assert_eq!(A2aStream::Events.stream_name(&p("myapp")), "MYAPP_EVENTS");
    }

    #[test]
    fn a2a_stream_dotted_prefix_normalizes() {
        assert_eq!(A2aStream::Events.stream_name(&p("vendor.a2a")), "VENDOR_A2A_EVENTS");
    }

    #[test]
    fn a2a_stream_subjects() {
        let patterns = A2aStream::Events.subject_patterns(&p("a2a"));
        assert_eq!(patterns, vec!["a2a.task.*.events.*"]);
        let dlq_patterns = A2aStream::PushDlq.subject_patterns(&p("a2a"));
        assert_eq!(
            dlq_patterns,
            vec!["a2a.push.dlq.*.*", "a2a.push.dlq.mirror.*.*"]
        );
    }

    #[test]
    fn a2a_stream_all_configs_covers_every_variant() {
        let configs = A2aStream::all_configs(&p("a2a"));
        assert_eq!(configs.len(), A2aStream::ALL.len());
        let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"A2A_EVENTS".to_string()));
        assert!(names.contains(&"A2A_PUSH_DLQ".to_string()));
    }

    #[test]
    fn to_subject_roundtrips() {
        use async_nats::subject::ToSubject;
        let prefix = p("a2a");
        let agent_id = aid("planner");
        let task_id = tid("t1");
        let req_id = rid("r1");

        assert_eq!(
            agent::MessageSendSubject::new(&prefix, &agent_id).to_subject().as_str(),
            "a2a.agent.planner.message.send"
        );
        assert_eq!(
            agent::TasksGetSubject::new(&prefix, &agent_id).to_subject().as_str(),
            "a2a.agent.planner.tasks.get"
        );
        assert_eq!(
            task::TaskEventsSubject::new(&prefix, &task_id, &req_id)
                .to_subject()
                .as_str(),
            "a2a.task.t1.events.r1"
        );
        assert_eq!(
            wildcards::AgentAllSubject::new(&prefix, &agent_id)
                .to_subject()
                .as_str(),
            "a2a.agent.planner.>"
        );
    }
}
