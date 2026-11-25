use serde::{Deserialize, Serialize};

/// The signal that ended a stopped command.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSignal {
    /// The command exited within the grace period after SIGTERM.
    Term,
    /// The grace period ran out and the command was killed.
    Kill,
}
