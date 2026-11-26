use serde::{Deserialize, Serialize};

use crate::{Digest, TenantId};

/// Client-facing request to look up a single snapshot's [`SnapshotListRow`](crate::SnapshotListRow) by digest.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotGetRequest {
    pub tenant: TenantId,
    pub digest: Digest,
}
