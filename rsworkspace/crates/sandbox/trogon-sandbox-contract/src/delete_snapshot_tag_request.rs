use serde::{Deserialize, Serialize};

use crate::{SnapshotTagName, TenantId};

/// Client-facing request to remove a named snapshot tag.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeleteSnapshotTagRequest {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
}
