use serde::{Deserialize, Serialize};

/// Whether the worker keeps a command's output for polling.
///
/// `StreamOnly` makes the worker keep no stdout: a poll returns empty stdout and only the last
/// [`OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES`] bytes of stderr, so a long-lived interactive process does not
/// grow worker memory.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputRetention {
    #[default]
    Buffer,
    StreamOnly,
}

impl OutputRetention {
    /// How many trailing stderr bytes a `StreamOnly` command keeps.
    pub const STREAM_ONLY_STDERR_TAIL_BYTES: usize = 64 * 1024;
}
