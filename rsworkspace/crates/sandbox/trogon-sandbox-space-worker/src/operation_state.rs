use std::fmt;

use serde::Deserialize;

/// Where a SPACE operation stands.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(from = "String")]
pub enum OperationState {
    /// The operation finished and took effect.
    Succeeded,
    /// The operation finished without taking effect.
    Failed,
    /// The operation was canceled.
    Canceled,
    /// The operation expired before it finished.
    Expired,
    /// The operation has not finished; the text is the state SPACE reported.
    Pending(String),
}

impl OperationState {
    /// The operation will not change state again.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending(_))
    }

    /// The state as SPACE spells it.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
            Self::Pending(state) => state,
        }
    }
}

impl From<String> for OperationState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            "expired" => Self::Expired,
            _ => Self::Pending(value),
        }
    }
}

impl fmt::Display for OperationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
