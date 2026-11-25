use serde::{Deserialize, Serialize};

/// Whether a command was killed for exceeding its deadline.
///
/// It serializes as the JSON boolean `timed_out` and is left off the wire when `WithinDeadline`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CommandTimeoutStatus {
    #[default]
    WithinDeadline,
    TimedOut,
}

impl CommandTimeoutStatus {
    pub const fn is_timed_out(self) -> bool {
        matches!(self, Self::TimedOut)
    }

    /// Whether this is `WithinDeadline`, the value left off the wire.
    pub const fn is_within_deadline(&self) -> bool {
        matches!(self, Self::WithinDeadline)
    }
}

impl From<bool> for CommandTimeoutStatus {
    fn from(value: bool) -> Self {
        if value { Self::TimedOut } else { Self::WithinDeadline }
    }
}

impl From<CommandTimeoutStatus> for bool {
    fn from(value: CommandTimeoutStatus) -> Self {
        value.is_timed_out()
    }
}
