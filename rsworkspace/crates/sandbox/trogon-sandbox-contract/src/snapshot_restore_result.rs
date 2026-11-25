use serde::{Deserialize, Serialize};

use crate::Digest;

/// The digest a worker reports after restoring a lease from a snapshot.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SnapshotRestoreResult {
    pub digest: Digest,
}
