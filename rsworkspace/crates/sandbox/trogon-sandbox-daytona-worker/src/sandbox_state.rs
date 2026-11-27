use serde::Deserialize;

/// The lifecycle state Daytona reports for a sandbox. A state this worker does not know is [`Self::Unknown`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Creating,
    Restoring,
    Destroyed,
    Destroying,
    Started,
    Stopped,
    Starting,
    Stopping,
    Error,
    BuildFailed,
    PendingBuild,
    BuildingSnapshot,
    PullingSnapshot,
    Archived,
    Archiving,
    #[serde(other)]
    Unknown,
}

impl SandboxState {
    /// A state the sandbox cannot leave on its own: waiting for it to start is pointless.
    pub fn is_failed(self) -> bool {
        matches!(
            self,
            Self::Error | Self::BuildFailed | Self::Destroyed | Self::Destroying
        )
    }
}

#[cfg(test)]
mod tests;
