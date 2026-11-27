use serde::Deserialize;

/// The lifecycle state Daytona reports for a snapshot. A state this worker does not know is [`Self::Unknown`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotState {
    Snapshotting,
    BuildPending,
    Building,
    Pending,
    Pulling,
    PendingValidation,
    Validating,
    Active,
    Inactive,
    Error,
    BuildFailed,
    Removing,
    #[serde(other)]
    Unknown,
}

impl SnapshotState {
    /// A state the snapshot cannot leave on its own: waiting for it to turn active is pointless.
    pub fn is_failed(self) -> bool {
        matches!(self, Self::Error | Self::BuildFailed)
    }
}
