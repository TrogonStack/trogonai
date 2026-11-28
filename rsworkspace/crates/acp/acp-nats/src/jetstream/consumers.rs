use std::time::Duration;

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

/// Durable name of the agent side's consumer on the COMMANDS stream.
///
/// The stream is already prefix-scoped (`{PREFIX}_COMMANDS`), so the name does
/// not repeat the prefix. Agent hosts serving one prefix share this consumer:
/// a restart resumes after the last acked command, and more than one host
/// splits commands instead of each receiving all of them (ADR#0055,
/// "Subscriptions and scaling").
pub const COMMANDS_CONSUMER_DURABLE: &str = "AGENT_COMMANDS";

/// How long JetStream waits for the agent side to settle a command before it
/// delivers the command again.
///
/// Long-running prompts send an in-progress ack every 15 s, so a long
/// `ack_wait` costs nothing while the host is reachable. It matters when acks
/// cannot get through: during a network partition the keepalives are lost, and
/// with the 30 s default a partition of a minute redelivered a running prompt
/// twice. Five minutes rides out partitions and NATS failovers well beyond
/// that.
///
/// The price is a slower redelivery after a host crashes without nacking: the
/// command waits up to five minutes before another host runs it. Clients wait
/// up to two hours for a prompt, so that delay stays inside their window. A
/// host that shuts down cleanly nacks what it holds, and those commands are
/// redelivered at once.
pub const COMMANDS_ACK_WAIT: Duration = Duration::from_secs(5 * 60);

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
