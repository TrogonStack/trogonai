use serde::{Deserialize, Serialize};

/// Whether a command launch must be cancelable.
///
/// It serializes as the JSON boolean `require_launch_cancellation` and is left off the wire when `NotRequired`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LaunchCancellation {
    #[default]
    NotRequired,
    Required,
}

impl LaunchCancellation {
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }

    /// Whether this is `NotRequired`, the value left off the wire.
    pub const fn is_not_required(&self) -> bool {
        matches!(self, Self::NotRequired)
    }
}

impl From<bool> for LaunchCancellation {
    fn from(value: bool) -> Self {
        if value { Self::Required } else { Self::NotRequired }
    }
}

impl From<LaunchCancellation> for bool {
    fn from(value: LaunchCancellation) -> Self {
        value.is_required()
    }
}
