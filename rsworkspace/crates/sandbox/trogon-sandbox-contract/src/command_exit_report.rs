use serde::{Deserialize, Serialize};

/// Whether the worker reported that a command exited.
///
/// It serializes as the JSON boolean `exited`. Brokers and workers that predate the exit marker leave it
/// `NotReported`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CommandExitReport {
    #[default]
    NotReported,
    Reported,
}

impl CommandExitReport {
    pub const fn is_reported(self) -> bool {
        matches!(self, Self::Reported)
    }
}

impl From<bool> for CommandExitReport {
    fn from(value: bool) -> Self {
        if value { Self::Reported } else { Self::NotReported }
    }
}

impl From<CommandExitReport> for bool {
    fn from(value: CommandExitReport) -> Self {
        value.is_reported()
    }
}
