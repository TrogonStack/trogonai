use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LiveSnapshotEntry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LiveArtifact {
    pub(super) entry: LiveSnapshotEntry,
    pub(super) reservation_sequence: u64,
    pub(super) projection_revision: u64,
}
