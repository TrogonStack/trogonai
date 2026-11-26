use serde::{Deserialize, Serialize};

/// Whether a page of logs stopped with retained chunks left.
///
/// It serializes as the JSON boolean `has_more`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum LogsContinuation {
    #[default]
    Complete,
    HasMore,
}

impl LogsContinuation {
    pub const fn has_more(self) -> bool {
        matches!(self, Self::HasMore)
    }
}

impl From<bool> for LogsContinuation {
    fn from(value: bool) -> Self {
        if value { Self::HasMore } else { Self::Complete }
    }
}

impl From<LogsContinuation> for bool {
    fn from(value: LogsContinuation) -> Self {
        value.has_more()
    }
}
