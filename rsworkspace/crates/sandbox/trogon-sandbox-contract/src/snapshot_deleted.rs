use serde::{Deserialize, Serialize};

use crate::{Digest, SnapshotGeneration, TenantId};

/// Announces that a snapshot artifact was removed, so its bytes stop counting against the tenant.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDeleted {
    pub digest: Digest,
    pub tenant: TenantId,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<SnapshotGeneration>,
}
