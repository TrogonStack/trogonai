use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{DriveId, DriveStorage, LeaseId, TenantId};

/// A persistent drive, its size, and the lease it is attached to, if any.
///
/// `storage` is flattened into the record, so an S3 drive carries `bucket` at the top level.
/// `created_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriveRecord {
    pub id: DriveId,
    pub tenant: TenantId,
    pub size_gib: u64,
    pub created_at: SystemTime,
    pub attached_lease: Option<LeaseId>,
    #[serde(flatten)]
    pub storage: DriveStorage,
}

impl DriveRecord {
    /// Drive size, in GiB, used when a request does not name one.
    pub const DEFAULT_SIZE_GIB: u64 = 100;
    /// Largest drive size, in GiB, a request may ask for.
    pub const MAX_SIZE_GIB: u64 = 1024;
}
