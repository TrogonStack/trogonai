use trogon_identity_types::SubjectPrincipal;

use crate::a2a_prefix::A2aPrefix;

/// `{prefix}.v1.tasks.*.{kind}.{id}.events`: every task event of one caller.
///
/// The filter of a consumer that serves one caller but cannot name the task yet,
/// such as the gateway's `message/stream` pump, which never sees the bootstrap
/// reply that names it. Narrowing on the principal keeps another caller's events
/// out of the consumer; concurrent streams of the same caller are still told apart
/// by `Trogon-Req-Id`.
#[derive(Debug)]
pub struct CallerTaskEventsSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
}

impl CallerTaskEventsSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
        }
    }
}

impl std::fmt::Display for CallerTaskEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.tasks.*.{}.events", self.prefix.as_str(), self.caller)
    }
}

impl async_nats::subject::ToSubject for CallerTaskEventsSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for CallerTaskEventsSubject {}

impl super::super::stream::StreamAssignment for CallerTaskEventsSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests;
