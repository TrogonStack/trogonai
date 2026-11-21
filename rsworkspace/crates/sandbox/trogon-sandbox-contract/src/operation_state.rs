use serde::{Deserialize, Serialize};

/// The operation lifecycle states accepted by the wire contract.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
    Expired,
}

impl OperationState {
    pub const ALL: [Self; 6] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Canceled,
        Self::Expired,
    ];

    /// Terminal states never change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled | Self::Expired)
    }

    /// `Running -> Running` permits takeover of a stale claim, while
    /// `Running -> Queued` permits retry after a transient failure.
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Running | Self::Canceled | Self::Expired)
                | (
                    Self::Running,
                    Self::Running | Self::Queued | Self::Succeeded | Self::Failed | Self::Canceled
                )
        )
    }
}
