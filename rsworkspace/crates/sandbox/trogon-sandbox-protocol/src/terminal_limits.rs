use crate::TerminalReplayRing;

/// Largest chunk a worker publishes as one output frame, in bytes before base64, so a frame stays well under the
/// message payload limit.
pub const MAX_TERMINAL_OUTPUT_CHUNK_BYTES: usize = 16 * 1024;

/// Most terminals a worker keeps open for one lease.
pub const MAX_TERMINALS_PER_LEASE: usize = 8;

/// The environment variable a worker reads to size each terminal's replay ring, in bytes.
///
/// Unset or unparsable means [`TerminalReplayRing::DEFAULT_CAPACITY`], `0` turns replay off, and larger values are
/// clamped to [`TerminalReplayRing::MAX_CAPACITY`].
pub const TERMINAL_REPLAY_BYTES_ENV: &str = "SANDBOX_TERMINAL_REPLAY_BYTES";

/// The replay ring size a configuration value asks for.
pub fn terminal_replay_bytes_from_value(value: Option<&str>) -> usize {
    value
        .and_then(|text| text.trim().parse::<usize>().ok())
        .map_or(TerminalReplayRing::DEFAULT_CAPACITY, |bytes| {
            bytes.min(TerminalReplayRing::MAX_CAPACITY)
        })
}

/// The replay ring size configured in [`TERMINAL_REPLAY_BYTES_ENV`].
pub fn terminal_replay_bytes_from_env() -> usize {
    terminal_replay_bytes_from_value(std::env::var(TERMINAL_REPLAY_BYTES_ENV).ok().as_deref())
}
