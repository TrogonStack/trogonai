//! A2A method inferred from the last dotted tokens of a NATS subject.
//!
//! The agent subscribes to `{prefix}.agents.{agent_id}.>` and dispatches based on
//! the suffix after `{prefix}.agents.{agent_id}`.

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

    /// Resolve the method from the full NATS subject string and the known
    /// `{prefix}.agents.{agent_id}` byte-length component.
    pub fn from_subject(subject: &str, prefix_len: usize) -> Option<Self> {
        let suffix = subject.get(prefix_len..)?.strip_prefix('.')?;
        Self::from_dotted_suffix(suffix)
    }

    /// Resolve the method from the already-extracted dotted-suffix form
    /// (e.g. `"message.send"`, `"tasks.get"`, `"card"`).
    ///
    /// Callers that have already split a subject into its
    /// `{prefix}.agents.{agent_id}.{method_dots}` parts — for example,
    /// [`crate::gateway_ingress::gateway_ingress_agent_and_method_dots`]
    /// returns `method_dots` as a `&str` — should use this entry point
    /// instead of rebuilding a full subject just to call
    /// [`A2aMethod::from_subject`].
    pub fn from_dotted_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "message.send" => Some(Self::MessageSend),
            "message.stream" => Some(Self::MessageStream),
            "tasks.get" => Some(Self::TasksGet),
            "tasks.list" => Some(Self::TasksList),
            "tasks.cancel" => Some(Self::TasksCancel),
            "tasks.resubscribe" => Some(Self::TasksResubscribe),
            "push.set" => Some(Self::PushNotificationSet),
            "push.get" => Some(Self::PushNotificationGet),
            "push.list" => Some(Self::PushNotificationList),
            "push.delete" => Some(Self::PushNotificationDelete),
            "card" => Some(Self::AgentCard),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
