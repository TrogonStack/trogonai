use serde::{Deserialize, Serialize};

use crate::{SnapshotTagName, TenantId};

/// Client-facing request to list the history of digests a named snapshot tag has pointed at.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTagHistoryRequest {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
}
