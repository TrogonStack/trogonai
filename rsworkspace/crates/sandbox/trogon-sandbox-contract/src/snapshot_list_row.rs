use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest, TenantId};

/// Row projection of a snapshot for listing and lookup responses, such as
/// [`SnapshotGetResponse`](crate::SnapshotGetResponse).
///
/// `parent` is populated when this snapshot is a layered delta of another digest, so a
/// [`SnapshotTreeResponse`](crate::SnapshotTreeResponse) can walk lineage without a separate lookup. It is omitted
/// when absent. `created_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotListRow {
    pub digest: Digest,
    pub tenant: TenantId,
    pub artifact_type: ArtifactType,
    pub created_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Digest>,
}
