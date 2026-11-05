//! A2A method named by the dotted terminal tokens of a request subject.
//!
//! Gateway ingress and agent delivery subjects end in the method's dotted form
//! (`message.send`, `tasks.get`, `card`, …); the subject types in
//! [`crate::nats::subjects`] parse and render it through this type.

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
    pub const ALL: [Self; 11] = [
        Self::MessageSend,
        Self::MessageStream,
        Self::TasksGet,
        Self::TasksList,
        Self::TasksCancel,
        Self::TasksResubscribe,
        Self::PushNotificationSet,
        Self::PushNotificationGet,
        Self::PushNotificationList,
        Self::PushNotificationDelete,
        Self::AgentCard,
    ];

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

    pub fn as_dotted_suffix(&self) -> &'static str {
        match self {
            Self::MessageSend => "message.send",
            Self::MessageStream => "message.stream",
            Self::TasksGet => "tasks.get",
            Self::TasksList => "tasks.list",
            Self::TasksCancel => "tasks.cancel",
            Self::TasksResubscribe => "tasks.resubscribe",
            Self::PushNotificationSet => "push.set",
            Self::PushNotificationGet => "push.get",
            Self::PushNotificationList => "push.list",
            Self::PushNotificationDelete => "push.delete",
            Self::AgentCard => "card",
        }
    }

    /// Resolve the method from its dotted subject terminal (e.g. `"message.send"`,
    /// `"tasks.get"`, `"card"`).
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

    /// Resolve the method from its JSON-RPC name (e.g. `"message/send"`,
    /// `"tasks/pushNotificationConfig/set"`), the inverse of [`Self::as_str`].
    pub fn from_jsonrpc_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.as_str() == name)
    }
}

#[cfg(test)]
mod tests;
