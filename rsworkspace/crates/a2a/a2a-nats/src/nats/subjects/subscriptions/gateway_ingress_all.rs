use crate::a2a_prefix::A2aPrefix;

/// `{prefix}.v1.gateway.*.*.*.>`: gateway-side subscription for every ingress request,
/// for every agent and every caller.
///
/// The `*.*` after the agent id covers the principal position, so the gateway parses
/// each request with [`crate::nats::subjects::gateway::GatewayIngressSubject`] to learn
/// the caller it relays for. The caller-scoped egress fanout carries fewer tokens and
/// never matches.
#[derive(Debug)]
pub struct GatewayIngressAllSubject {
    prefix: A2aPrefix,
}

impl GatewayIngressAllSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for GatewayIngressAllSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.gateway.*.*.*.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for GatewayIngressAllSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for GatewayIngressAllSubject {}

impl super::super::stream::StreamAssignment for GatewayIngressAllSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests;
