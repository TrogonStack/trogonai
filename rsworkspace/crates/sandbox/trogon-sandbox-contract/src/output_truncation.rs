use serde::{Deserialize, Serialize};

/// Whether retention dropped part of a command's output.
///
/// It serializes as the JSON boolean `truncated`. In a [`CommandOutput`](crate::CommandOutput) it is left off the
/// wire when `Complete`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum OutputTruncation {
    #[default]
    Complete,
    Truncated,
}

impl OutputTruncation {
    pub const fn is_truncated(self) -> bool {
        matches!(self, Self::Truncated)
    }

    /// Whether this is `Complete`, the value left off the wire.
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}

impl From<bool> for OutputTruncation {
    fn from(value: bool) -> Self {
        if value { Self::Truncated } else { Self::Complete }
    }
}

impl From<OutputTruncation> for bool {
    fn from(value: OutputTruncation) -> Self {
        value.is_truncated()
    }
}
