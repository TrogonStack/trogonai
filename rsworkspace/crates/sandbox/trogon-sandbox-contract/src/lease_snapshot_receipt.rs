use serde::{Deserialize, Serialize};

use crate::OciArtifactRef;

/// Where a captured snapshot lives and how big it is.
///
/// `reference` can be used as a new lease's snapshot image unchanged.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseSnapshotReceipt {
    pub reference: OciArtifactRef,
    pub bytes: u64,
}
