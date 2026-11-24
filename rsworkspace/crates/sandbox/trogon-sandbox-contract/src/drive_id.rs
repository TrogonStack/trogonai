use serde::{Deserialize, Serialize};

use crate::{DriveName, TenantId};

/// A tenant-scoped persistent drive name.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DriveId {
    pub tenant: TenantId,
    pub name: DriveName,
}
