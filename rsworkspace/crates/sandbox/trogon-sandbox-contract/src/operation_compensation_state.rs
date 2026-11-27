use serde::{Deserialize, Serialize};

/// Whether the cleanup of a canceled create is still owed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationCompensationState {
    Pending,
    Cleaned,
}
