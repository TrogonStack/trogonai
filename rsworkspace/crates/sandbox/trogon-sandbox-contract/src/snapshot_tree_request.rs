use serde::{Deserialize, Serialize};

use crate::{Digest, TenantId};

/// Client-facing request to walk a snapshot digest's lineage tree.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTreeRequest {
    pub tenant: TenantId,
    pub digest: Digest,
}
