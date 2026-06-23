    use super::*;

    fn prefix_len(prefix: &str, agent_id: &str) -> usize {
        format!("{prefix}.agents.{agent_id}").len()
    }

    #[test]
    fn message_send_resolves() {
        let pl = prefix_len("a2a", "planner");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.planner.message.send", pl),
            Some(A2aMethod::MessageSend)
        );
    }

    #[test]
    fn message_stream_resolves() {
        let pl = prefix_len("a2a", "planner");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.planner.message.stream", pl),
            Some(A2aMethod::MessageStream)
        );
    }

    #[test]
    fn tasks_get_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.tasks.get", pl),
            Some(A2aMethod::TasksGet)
        );
    }

    #[test]
    fn tasks_list_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.tasks.list", pl),
            Some(A2aMethod::TasksList)
        );
    }

    #[test]
    fn tasks_cancel_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.tasks.cancel", pl),
            Some(A2aMethod::TasksCancel)
        );
    }

    #[test]
    fn tasks_resubscribe_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.tasks.resubscribe", pl),
            Some(A2aMethod::TasksResubscribe)
        );
    }

    #[test]
    fn push_set_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.push.set", pl),
            Some(A2aMethod::PushNotificationSet)
        );
    }

    #[test]
    fn push_get_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.push.get", pl),
            Some(A2aMethod::PushNotificationGet)
        );
    }

    #[test]
    fn push_list_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.push.list", pl),
            Some(A2aMethod::PushNotificationList)
        );
    }

    #[test]
    fn push_delete_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.push.delete", pl),
            Some(A2aMethod::PushNotificationDelete)
        );
    }

    #[test]
    fn agent_card_resolves() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agents.bot.card", pl),
            Some(A2aMethod::AgentCard)
        );
    }

    #[test]
    fn unknown_suffix_returns_none() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(A2aMethod::from_subject("a2a.agents.bot.unknown.method", pl), None);
    }

    #[test]
    fn too_short_subject_returns_none() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(A2aMethod::from_subject("a2a.agents.bot", pl), None);
    }

    #[test]
    fn as_str_covers_every_variant() {
        let pairs = [
            (A2aMethod::MessageSend, "message/send"),
            (A2aMethod::MessageStream, "message/stream"),
            (A2aMethod::TasksGet, "tasks/get"),
            (A2aMethod::TasksList, "tasks/list"),
            (A2aMethod::TasksCancel, "tasks/cancel"),
            (A2aMethod::TasksResubscribe, "tasks/resubscribe"),
            (A2aMethod::PushNotificationSet, "tasks/pushNotificationConfig/set"),
            (A2aMethod::PushNotificationGet, "tasks/pushNotificationConfig/get"),
            (A2aMethod::PushNotificationList, "tasks/pushNotificationConfig/list"),
            (A2aMethod::PushNotificationDelete, "tasks/pushNotificationConfig/delete"),
            (A2aMethod::AgentCard, "agent/card"),
        ];
        for (method, expected) in pairs {
            assert_eq!(method.as_str(), expected);
        }
    }
