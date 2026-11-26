use serde::{Deserialize, Serialize};

use crate::{Digest, TenantId};

/// Client-facing request to delete a snapshot by digest. It is idempotent: a repeat reports `not_found`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDeleteRequest {
    pub tenant: TenantId,
    pub digest: Digest,
}
