use serde::{Deserialize, Serialize};

use crate::{Digest, LeaseId, SnapshotAccountingDisposition, SnapshotAccountingRefusal, SnapshotGeneration, TenantId};

/// The reply to a snapshot accounting request, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SnapshotAccountingResponse {
    Committed {
        tenant: TenantId,
        digest: Digest,
        lease_id: LeaseId,
        generation: SnapshotGeneration,
        disposition: SnapshotAccountingDisposition,
    },
    Refused {
        reason: SnapshotAccountingRefusal,
    },
}
