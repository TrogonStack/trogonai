/// A2A method inferred from the last dotted tokens of a NATS subject.
///
/// The agent subscribes to `{prefix}.agent.{agent_id}.>` and dispatches based on
/// the suffix after `{prefix}.agent.{agent_id}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2aMethod {
    MessageSend,
    MessageStream,
    TasksGet,
    TasksList,
    TasksCancel,
    TasksResubscribe,
    PushNotificationSet,
    PushNotificationGet,
    PushNotificationList,
    PushNotificationDelete,
    AgentCard,
}

impl A2aMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MessageSend => "message/send",
            Self::MessageStream => "message/stream",
            Self::TasksGet => "tasks/get",
            Self::TasksList => "tasks/list",
            Self::TasksCancel => "tasks/cancel",
            Self::TasksResubscribe => "tasks/resubscribe",
            Self::PushNotificationSet => "tasks/pushNotificationConfig/set",
            Self::PushNotificationGet => "tasks/pushNotificationConfig/get",
            Self::PushNotificationList => "tasks/pushNotificationConfig/list",
            Self::PushNotificationDelete => "tasks/pushNotificationConfig/delete",
            Self::AgentCard => "agent/card",
        }
    }

    /// Resolve the method from the full NATS subject string and the known prefix/agent_id
    /// component length.
    ///
    /// `prefix_len` is the byte length of `{prefix}.agent.{agent_id}` (without trailing dot).
    pub fn from_subject(subject: &str, prefix_len: usize) -> Option<Self> {
        let suffix = subject.get(prefix_len..)?.strip_prefix('.')?;
        match suffix {
            "message.send" => Some(Self::MessageSend),
            "message.stream" => Some(Self::MessageStream),
            "tasks.get" => Some(Self::TasksGet),
            "tasks.list" => Some(Self::TasksList),
            "tasks.cancel" => Some(Self::TasksCancel),
            "tasks.resubscribe" => Some(Self::TasksResubscribe),
            "tasks.push_notification_config.set" => Some(Self::PushNotificationSet),
            "tasks.push_notification_config.get" => Some(Self::PushNotificationGet),
            "tasks.push_notification_config.list" => Some(Self::PushNotificationList),
            "tasks.push_notification_config.delete" => Some(Self::PushNotificationDelete),
            "agent.card" => Some(Self::AgentCard),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_len(prefix: &str, agent_id: &str) -> usize {
        format!("{prefix}.agent.{agent_id}").len()
    }

    #[test]
    fn message_send() {
        let pl = prefix_len("a2a", "planner");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.planner.message.send", pl),
            Some(A2aMethod::MessageSend)
        );
    }

    #[test]
    fn message_stream() {
        let pl = prefix_len("a2a", "planner");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.planner.message.stream", pl),
            Some(A2aMethod::MessageStream)
        );
    }

    #[test]
    fn tasks_get() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.get", pl),
            Some(A2aMethod::TasksGet)
        );
    }

    #[test]
    fn tasks_list() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.list", pl),
            Some(A2aMethod::TasksList)
        );
    }

    #[test]
    fn tasks_cancel() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.cancel", pl),
            Some(A2aMethod::TasksCancel)
        );
    }

    #[test]
    fn tasks_resubscribe() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.resubscribe", pl),
            Some(A2aMethod::TasksResubscribe)
        );
    }

    #[test]
    fn push_set() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.push_notification_config.set", pl),
            Some(A2aMethod::PushNotificationSet)
        );
    }

    #[test]
    fn push_get() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.push_notification_config.get", pl),
            Some(A2aMethod::PushNotificationGet)
        );
    }

    #[test]
    fn push_list() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.push_notification_config.list", pl),
            Some(A2aMethod::PushNotificationList)
        );
    }

    #[test]
    fn push_delete() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.tasks.push_notification_config.delete", pl),
            Some(A2aMethod::PushNotificationDelete)
        );
    }

    #[test]
    fn agent_card() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(
            A2aMethod::from_subject("a2a.agent.bot.agent.card", pl),
            Some(A2aMethod::AgentCard)
        );
    }

    #[test]
    fn unknown_suffix_returns_none() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(A2aMethod::from_subject("a2a.agent.bot.unknown.method", pl), None);
    }

    #[test]
    fn too_short_subject_returns_none() {
        let pl = prefix_len("a2a", "bot");
        assert_eq!(A2aMethod::from_subject("a2a.agent.bot", pl), None);
    }

    #[test]
    fn custom_prefix_resolves() {
        let pl = prefix_len("myapp", "worker");
        assert_eq!(
            A2aMethod::from_subject("myapp.agent.worker.message.send", pl),
            Some(A2aMethod::MessageSend)
        );
    }
}
