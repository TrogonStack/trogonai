use serde::{Deserialize, Serialize};

/// How far an owned cleanup has progressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Cleanup,
    Absent,
}
