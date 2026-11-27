use std::time::SystemTime;

use serde::Serialize;
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, SnapshotPublished, TenantId};

use crate::unix_seconds::unix_seconds;

/// A snapshot publication on the dashboard stream.
#[derive(Debug, Serialize)]
pub(crate) struct SnapshotPublishedRow<'a> {
    digest: &'a Digest,
    artifact_type: ArtifactType,
    tenant: &'a TenantId,
    published_at: u64,
    bytes: u64,
    lease_id: &'a LeaseId,
}

impl<'a> From<&'a SnapshotPublished> for SnapshotPublishedRow<'a> {
    fn from(event: &'a SnapshotPublished) -> Self {
        Self {
            digest: &event.digest,
            artifact_type: event.artifact_type,
            tenant: &event.tenant,
            published_at: unix_seconds(SystemTime::now()),
            bytes: event.bytes,
            lease_id: &event.lease_id,
        }
    }
}
