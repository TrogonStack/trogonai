use trogon_identity_types::SubjectPrincipal;

use crate::a2a_prefix::A2aPrefix;

/// `{prefix}.v1.gateway.{kind}.{id}.events`: the caller-scoped fanout the gateway
/// forwards a caller's task events on.
///
/// Scoped to the caller rather than the request (ADR#0055): the request an event
/// belongs to travels in `Trogon-Req-Id`. The principal is the one the task event
/// subject named (ADR#0082), so a caller subscribed to its own fanout only ever
/// sees its own tasks. The shape never matches gateway ingress, which carries an
/// agent id before the principal and a method after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEgressEventsSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
}

impl GatewayEgressEventsSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
        }
    }

    pub fn caller(&self) -> &SubjectPrincipal {
        &self.caller
    }
}

impl std::fmt::Display for GatewayEgressEventsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.gateway.{}.events", self.prefix.as_str(), self.caller)
    }
}

impl async_nats::subject::ToSubject for GatewayEgressEventsSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Publishable for GatewayEgressEventsSubject {}

impl super::super::stream::StreamAssignment for GatewayEgressEventsSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests;
