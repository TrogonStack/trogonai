//! Wire-string to [`A2aMethod`] parsing for fixture inputs.
//!
//! `A2aMethod` itself only exposes subject-suffix parsing
//! (`from_dotted_suffix`, e.g. `"message.send"`), not the JSON-RPC wire
//! method string (`"message/send"`) that fixtures naturally write. This
//! module bridges that gap without touching `a2a-nats`.

use a2a_nats::A2aMethod;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown a2a request method '{0}'")]
pub struct RequestMethodError(String);

pub fn parse_request_method(raw: &str) -> Result<A2aMethod, RequestMethodError> {
    match raw {
        "message/send" => Ok(A2aMethod::MessageSend),
        "message/stream" => Ok(A2aMethod::MessageStream),
        "tasks/get" => Ok(A2aMethod::TasksGet),
        "tasks/list" => Ok(A2aMethod::TasksList),
        "tasks/cancel" => Ok(A2aMethod::TasksCancel),
        "tasks/resubscribe" => Ok(A2aMethod::TasksResubscribe),
        "tasks/pushNotificationConfig/set" => Ok(A2aMethod::PushNotificationSet),
        "tasks/pushNotificationConfig/get" => Ok(A2aMethod::PushNotificationGet),
        "tasks/pushNotificationConfig/list" => Ok(A2aMethod::PushNotificationList),
        "tasks/pushNotificationConfig/delete" => Ok(A2aMethod::PushNotificationDelete),
        "agent/card" => Ok(A2aMethod::AgentCard),
        other => Err(RequestMethodError(other.to_string())),
    }
}

#[cfg(test)]
mod tests;
