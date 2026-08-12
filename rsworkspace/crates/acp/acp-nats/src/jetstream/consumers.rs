use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

use crate::acp_prefix::AcpPrefix;
use crate::session_id::AcpSessionId;

/// Consumers below filter on a session-scoped subject, so they see every
/// in-flight request's traffic for that session and the caller demuxes on the
/// `Jsonrpc-Id` header (ADR#0055).
///
/// `DeliverPolicy::New` rather than `All`: the consumer is created before the
/// request is published, so nothing is missed, and a session-wide filter under
/// `All` would replay the session's entire history on every new request.
pub fn prompt_notifications_consumer(prefix: &AcpPrefix, session_id: &AcpSessionId) -> Config {
    let pfx = prefix.as_str();
    let sid = session_id.as_str();
    Config {
        filter_subject: format!("{pfx}.v1.session.{sid}.agent.update"),
        deliver_policy: DeliverPolicy::New,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

pub fn response_consumer(prefix: &AcpPrefix, session_id: &AcpSessionId) -> Config {
    let pfx = prefix.as_str();
    let sid = session_id.as_str();
    Config {
        filter_subject: format!("{pfx}.v1.session.{sid}.agent.response"),
        deliver_policy: DeliverPolicy::New,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

/// Observer consumer for the COMMANDS stream.
///
/// Acks messages for audit persistence. No filter needed — the stream-level
/// subject list already scopes to session-scoped commands only.
pub fn commands_observer() -> Config {
    Config {
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
