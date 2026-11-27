use serde::Serialize;
use trogon_sandbox_contract::{ArtifactType, Digest, SnapshotTagRecord, TenantId};

use crate::unix_seconds::unix_seconds;

/// One snapshot tag as the dashboard lists it.
#[derive(Debug, Serialize)]
pub(crate) struct TagRow<'a> {
    tenant: &'a TenantId,
    pub(crate) name: String,
    digest: &'a Digest,
    artifact_type: ArtifactType,
    set_at: u64,
}

impl<'a> TagRow<'a> {
    pub(crate) fn new(tenant: &'a TenantId, name: String, record: &'a SnapshotTagRecord) -> Self {
        Self {
            tenant,
            name,
            digest: &record.digest,
            artifact_type: record.artifact_type,
            set_at: unix_seconds(record.set_at),
        }
    }
}
