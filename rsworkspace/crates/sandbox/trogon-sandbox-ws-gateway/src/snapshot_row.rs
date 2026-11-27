use serde::Serialize;
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, LiveSnapshotEntry, SnapshotKvRecord, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use crate::unix_seconds::unix_seconds;

/// One snapshot as the dashboard lists it.
#[derive(Debug, Serialize)]
pub(crate) struct SnapshotRow<'a> {
    pub(crate) digest: &'a Digest,
    artifact_type: ArtifactType,
    tenant: &'a TenantId,
    published_at: Option<u64>,
    bytes: Option<u64>,
    registry: &'a str,
    repository: &'a str,
    lease_id: &'a LeaseId,
    kv_key: String,
}

impl<'a> SnapshotRow<'a> {
    pub(crate) fn new(record: &'a SnapshotKvRecord, live: Option<&LiveSnapshotEntry>) -> Self {
        Self {
            digest: &record.digest,
            artifact_type: record.artifact_type,
            tenant: &record.tenant,
            published_at: live.map(|entry| unix_seconds(entry.published_at)),
            bytes: live.map(|entry| entry.bytes),
            registry: &record.registry,
            repository: &record.repository,
            lease_id: &record.lease_id,
            kv_key: tenant_scoped_kv_key(&record.tenant, record.digest.as_str()),
        }
    }
}
