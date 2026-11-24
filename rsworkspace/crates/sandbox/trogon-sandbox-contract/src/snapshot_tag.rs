use serde::{Deserialize, Serialize};

use crate::{SnapshotTagName, TenantId};

/// A tenant-scoped name that points at a snapshot digest.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SnapshotTag {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
}
