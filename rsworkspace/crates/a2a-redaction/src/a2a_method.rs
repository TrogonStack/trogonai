use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

    pub fn all() -> &'static [Self] {
        &[
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
        ]
    }
}

impl fmt::Display for A2aMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for A2aMethod {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::all()
            .iter()
            .copied()
            .find(|method| method.as_str() == value)
            .ok_or_else(|| format!("unknown A2A method: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_display_parse() {
        for method in A2aMethod::all() {
            let parsed = method.as_str().parse::<A2aMethod>().unwrap();
            assert_eq!(parsed, *method);
        }
    }
}
