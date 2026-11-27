use serde::{Deserialize, Serialize};

use crate::AttemptLedgerError;

/// Where one dispatch attempt stands in its durable record.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    /// The backend allocation was started and has not finished.
    Starting,
    /// The backend allocation finished and the lease is usable.
    Ready,
    /// A cleanup was requested and the backend teardown has not finished.
    CleanupRequested,
    /// The backend was torn down.
    Cleaned,
}

impl AttemptPhase {
    pub(crate) fn code(self) -> i64 {
        match self {
            Self::Starting => 0,
            Self::Ready => 1,
            Self::CleanupRequested => 2,
            Self::Cleaned => 3,
        }
    }

    pub(crate) fn from_code(value: i64) -> Result<Self, AttemptLedgerError> {
        match value {
            0 => Ok(Self::Starting),
            1 => Ok(Self::Ready),
            2 => Ok(Self::CleanupRequested),
            3 => Ok(Self::Cleaned),
            _ => Err(AttemptLedgerError::InvalidPhase),
        }
    }
}
