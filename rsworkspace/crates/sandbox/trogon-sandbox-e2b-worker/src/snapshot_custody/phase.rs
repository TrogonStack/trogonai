use serde::{Deserialize, Serialize};

/// Where a snapshot is in its ownership lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CustodyPhase {
    Capturing,
    Owned,
    Kept,
    Cleanup,
    Deleted,
}
