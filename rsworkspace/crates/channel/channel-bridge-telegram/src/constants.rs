/// Durable consumer identity on the inbound stream. JetStream keys the ack
/// floor by this string, so it is deployment state rather than a build
/// artifact name: a literal here means a future crate rename cannot silently
/// strand every deployment's position in the stream.
pub const INBOUND_DURABLE: &str = "channel-bridge-telegram";

/// The Telegram limit for a single message, counted in UTF-16 code units.
pub const TEXT_CHUNK_LIMIT: usize = 4096;

/// What the bridge says back when a command has nothing else to do. A reset
/// with no follow-up prompt produces no agent output, so without this the user
/// gets silence.
pub const NEW_SESSION_ACKNOWLEDGEMENT: &str = "Started a new session.";
