use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{ArtifactType, Digest, EnvDigest, LeaseId, TenantId};

/// The catalog record a snapshot builder writes for each published snapshot.
///
/// `parent`, `created_at`, and `env_digest` are omitted when absent and default when an older record lacks them.
/// `created_at` uses serde's `SystemTime` encoding, an object of `secs_since_epoch` and `nanos_since_epoch`.
/// `env_digest` is the environment the builder read from the snapshot's manifest, and a restore is placed by it,
/// never by what a request claims.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotKvRecord {
    pub lease_id: LeaseId,
    pub tenant: TenantId,
    pub digest: Digest,
    pub artifact_type: ArtifactType,
    pub registry: String,
    pub repository: String,
    /// The snapshot this one was captured from. Absent for a root snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_digest: Option<EnvDigest>,
}
