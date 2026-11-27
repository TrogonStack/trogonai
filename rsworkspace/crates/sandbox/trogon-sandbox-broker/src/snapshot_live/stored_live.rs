use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LiveSnapshotEntry;

use super::snapshot_live_error::SnapshotLiveError;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct StoredLive {
    #[serde(flatten)]
    pub(super) entry: LiveSnapshotEntry,
    #[serde(default)]
    pub(super) revision: u64,
    #[serde(default)]
    pub(super) deleted: bool,
}

pub(super) fn replace(current: &StoredLive, next: &StoredLive) -> Result<bool, SnapshotLiveError> {
    if current.revision > next.revision || current == next {
        return Ok(false);
    }
    if current.revision == next.revision && next.revision != 0 {
        return Err(SnapshotLiveError::ConflictingRevision);
    }
    Ok(true)
}
