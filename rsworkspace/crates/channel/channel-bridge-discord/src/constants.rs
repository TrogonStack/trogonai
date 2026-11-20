/// The channel token of every endpoint this bridge builds. It is the first
/// component of a KV key other channels' bridges write beside, so it names the
/// platform and nothing about this process.
pub const CHANNEL: &str = "discord";

/// Durable consumer identity on the inbound stream. JetStream keys the ack
/// floor by this string, so it is deployment state rather than a build
/// artifact name.
pub const INBOUND_DURABLE: &str = "channel-bridge-discord";

/// Discord's limit for one message's content, in characters (Unicode scalar
/// values, which is how both Discord and `twilight-validate` count it).
pub const TEXT_CHUNK_LIMIT: usize = 2000;

/// What the bridge says back when a command has nothing else to do. A reset
/// with no follow-up prompt produces no agent output, so without this the user
/// gets silence.
pub const NEW_SESSION_ACKNOWLEDGEMENT: &str = "Started a new session.";
