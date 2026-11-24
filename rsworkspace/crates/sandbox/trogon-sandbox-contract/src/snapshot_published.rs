use serde::{Deserialize, Serialize};

use crate::{ArtifactType, CorrelationId, Digest, LeaseId, SnapshotGeneration, SnapshotPublishedTag, TenantId};

/// Announces that a lease's snapshot was pushed to the registry.
///
/// `generation`, `correlation_id`, and `tag` are omitted when absent; older builders send none of
/// them.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPublished {
    pub lease_id: LeaseId,
    pub digest: Digest,
    pub artifact_type: ArtifactType,
    pub tenant: TenantId,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<SnapshotGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    /// The tag the builder moves to this snapshot right after the event. The move can still lose to
    /// a newer snapshot, so a reader applies the same newer-wins rule on `set_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<SnapshotPublishedTag>,
}
