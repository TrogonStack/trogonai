use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest, RetentionPolicy, ServiceAccountId};

/// The digest a snapshot tag currently points at, who set it, and when.
///
/// `set_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and
/// `nanos_since_epoch`. `retention` is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotTagRecord {
    pub digest: Digest,
    pub artifact_type: ArtifactType,
    pub set_by: ServiceAccountId,
    pub set_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<RetentionPolicy>,
}
