use a2a_nats::A2aMethod;
use trogon_audit::ReadOnly;

/// The methods the gateway's contract declares read-only (ADR#0086 Decision 3).
///
/// A method belongs here only when serving it has no effect on the target agent.
/// The match is exhaustive on purpose: a new method has to be placed, and one
/// nobody placed would otherwise be audited best-effort by omission. Resubscribe
/// is left out because it opens a delivery pump on the gateway for the caller.
pub fn declared_read_only(method: &A2aMethod) -> ReadOnly {
    match method {
        A2aMethod::TasksGet
        | A2aMethod::TasksList
        | A2aMethod::PushNotificationGet
        | A2aMethod::PushNotificationList
        | A2aMethod::AgentCard => ReadOnly::Declared,
        A2aMethod::MessageSend
        | A2aMethod::MessageStream
        | A2aMethod::TasksCancel
        | A2aMethod::TasksResubscribe
        | A2aMethod::PushNotificationSet
        | A2aMethod::PushNotificationDelete => ReadOnly::Undeclared,
    }
}

#[cfg(test)]
mod tests;
