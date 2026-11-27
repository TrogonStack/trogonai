use trogon_std::env::ReadEnv;

use crate::TerminalReplayRing;
pub use crate::constants::{MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, TERMINAL_REPLAY_BYTES_ENV};

/// The replay ring size a configuration value asks for.
pub fn terminal_replay_bytes_from_value(value: Option<&str>) -> usize {
    value
        .and_then(|text| text.trim().parse::<usize>().ok())
        .map_or(TerminalReplayRing::DEFAULT_CAPACITY, |bytes| {
            bytes.min(TerminalReplayRing::MAX_CAPACITY)
        })
}

/// The replay ring size configured in [`TERMINAL_REPLAY_BYTES_ENV`].
pub fn terminal_replay_bytes_from_env(env: &impl ReadEnv) -> usize {
    terminal_replay_bytes_from_value(env.var(TERMINAL_REPLAY_BYTES_ENV).ok().as_deref())
}
