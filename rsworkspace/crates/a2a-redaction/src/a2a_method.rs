use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A2A wire methods. The serde wire form is the canonical slash-delimited
/// string (e.g. `message/send`), matching `as_str` / `Display` / `FromStr`
/// so the same value round-trips across JSON and string parsing without
/// silently switching representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum A2aMethod {
    #[serde(rename = "message/send")]
    MessageSend,
    #[serde(rename = "message/stream")]
    MessageStream,
    #[serde(rename = "tasks/get")]
    TasksGet,
    #[serde(rename = "tasks/list")]
    TasksList,
    #[serde(rename = "tasks/cancel")]
    TasksCancel,
    #[serde(rename = "tasks/resubscribe")]
    TasksResubscribe,
    #[serde(rename = "tasks/pushNotificationConfig/set")]
    PushNotificationSet,
    #[serde(rename = "tasks/pushNotificationConfig/get")]
    PushNotificationGet,
    #[serde(rename = "tasks/pushNotificationConfig/list")]
    PushNotificationList,
    #[serde(rename = "tasks/pushNotificationConfig/delete")]
    PushNotificationDelete,
    #[serde(rename = "agent/card")]
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown A2A method: {value}")]
pub struct ParseA2aMethodError {
    pub value: String,
}

impl FromStr for A2aMethod {
    type Err = ParseA2aMethodError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::all()
            .iter()
            .copied()
            .find(|method| method.as_str() == value)
            .ok_or_else(|| ParseA2aMethodError {
                value: value.to_owned(),
            })
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

    #[test]
    fn serde_uses_canonical_wire_strings() {
        let method = A2aMethod::MessageSend;
        let wire = serde_json::to_string(&method).expect("serialize");
        assert_eq!(wire, "\"message/send\"");
        let back: A2aMethod = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(back, method);
    }

    #[test]
    fn parse_error_includes_unknown_value() {
        let err = "not/real".parse::<A2aMethod>().unwrap_err();
        assert_eq!(err.value, "not/real");
        assert!(err.to_string().contains("not/real"));
    }
}
