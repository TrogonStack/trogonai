use std::time::Duration;

pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// How long to keep retrying to recreate a lost consumer. Tied to
/// [`COMMANDS_ACK_WAIT`](acp_nats::jetstream::consumers::COMMANDS_ACK_WAIT): past it, JetStream has redelivered every command this
/// host held, so staying up buys nothing over a restart.
pub const RESUBSCRIBE_DEADLINE: Duration = acp_nats::jetstream::consumers::COMMANDS_ACK_WAIT;
pub const RESUBSCRIBE_MIN_DELAY: Duration = Duration::from_millis(250);
pub const RESUBSCRIBE_MAX_DELAY: Duration = Duration::from_secs(10);
