use std::time::Duration;

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
