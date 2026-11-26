use serde::{Deserialize, Serialize};

use crate::{DriveId, TenantId};

/// Client-facing request to delete a drive.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeleteDriveRequest {
    pub tenant: TenantId,
    pub drive_id: DriveId,
}
