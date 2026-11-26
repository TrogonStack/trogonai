use serde::{Deserialize, Serialize};

use crate::{Digest, SnapshotTagName, TenantId};

/// Client-facing request to point a named snapshot tag at a digest.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetSnapshotTagRequest {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
    pub digest: Digest,
}
