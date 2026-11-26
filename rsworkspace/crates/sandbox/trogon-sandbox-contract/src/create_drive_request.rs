use serde::{Deserialize, Serialize};

use crate::{DriveId, DriveStorage, TenantId};

/// Client-facing request to create a drive.
///
/// `size_gib` is `null` or absent when the caller does not name a size; the broker then applies its default of 100
/// GiB and rejects more than 1024 GiB. `storage` is flattened into the request, so an S3 drive carries `bucket` at the
/// top level.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateDriveRequest {
    pub tenant: TenantId,
    pub drive_id: DriveId,
    pub size_gib: Option<u64>,
    #[serde(flatten)]
    pub storage: DriveStorage,
}
