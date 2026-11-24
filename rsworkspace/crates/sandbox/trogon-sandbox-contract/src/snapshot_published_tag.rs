use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{RetentionPolicy, SnapshotTagName};

/// A tag the snapshot builder moves to a published snapshot, in the event's tenant, with the time
/// tag moves are ordered by.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPublishedTag {
    pub name: SnapshotTagName,
    pub set_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionPolicy>,
}
