use async_nats::jetstream::consumer::pull::Config;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};

mod constants;

pub use constants::{COMMANDS_ACK_WAIT, COMMANDS_CONSUMER_DURABLE};

use crate::acp_prefix::AcpPrefix;
use crate::session_id::AcpSessionId;

/// Consumers below filter on a session-scoped subject, so they see every
/// in-flight request's traffic for that session and the caller demuxes on the
/// `Jsonrpc-Id` header (ADR#0055).
///
/// `DeliverPolicy::New` rather than `All`: the consumer is created before the
/// request is published, so nothing is missed, and a session-wide filter under
/// `All` would replay the session's entire history on every new request.
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

/// Agent-side consumer for the COMMANDS stream.
///
/// COMMANDS is a `Limits` stream kept for `max_age`, so a consumer that starts
/// from the beginning would replay every past prompt and cancel each time the
/// agent side starts. The consumer is durable instead: acked commands are
/// never delivered again, and commands published while no agent host is
/// running wait in the stream until one starts.
///
/// `DeliverPolicy::New` only applies when the durable is first created, so a
/// host meeting an existing stream for the first time does not replay its
/// history either. No filter needed — the stream-level subject list already
/// scopes to session-scoped commands only.
pub fn agent_commands_consumer() -> Config {
    Config {
        durable_name: Some(COMMANDS_CONSUMER_DURABLE.to_owned()),
        deliver_policy: DeliverPolicy::New,
        ack_policy: AckPolicy::Explicit,
        ack_wait: COMMANDS_ACK_WAIT,
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
