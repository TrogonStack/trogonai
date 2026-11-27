/// The channel token of every endpoint this bridge builds. It is the first
/// component of a KV key other channels' bridges write beside, so it names the
/// platform and nothing about this process.
pub const CHANNEL: &str = "discord";

/// Durable consumer identity on the inbound stream. JetStream keys the ack
/// floor by this string, so it is deployment state rather than a build
/// artifact name.
pub const INBOUND_DURABLE: &str = "channel-bridge-discord";

/// Discord's limit for one message's content. Discord documents it in
/// characters; chunks are measured in UTF-16 code units, which is never fewer,
/// so a chunk fits however Discord counts.
pub const TEXT_CHUNK_LIMIT: usize = 2000;
