use std::fmt;

use serde::Deserialize;

/// The lifecycle state SPACE reports for a sandbox.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(from = "String")]
pub enum SandboxState {
    /// Running and ready for commands.
    Running,
    /// Frozen in memory.
    Paused,
    /// Suspended to durable storage.
    Suspended,
    /// Being deleted.
    Deleting,
    /// Deleted.
    Deleted,
    /// Moving between states; the text is the state SPACE reported.
    Transitional(String),
    /// Any other state, such as a failed one; the text is the state SPACE reported.
    Other(String),
}

impl SandboxState {
    /// The state as SPACE spells it.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Suspended => "suspended",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Transitional(state) | Self::Other(state) => state,
        }
    }

    /// The sandbox is on its way to another state.
    pub fn is_transitional(&self) -> bool {
        matches!(self, Self::Transitional(_))
    }
}

impl From<String> for SandboxState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "suspended" => Self::Suspended,
            "deleting" => Self::Deleting,
            "deleted" => Self::Deleted,
            "creating" | "starting" | "pausing" | "resuming" | "suspending" | "restoring" | "migrating"
            | "stopping" => Self::Transitional(value),
            _ => Self::Other(value),
        }
    }
}

impl fmt::Display for SandboxState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
