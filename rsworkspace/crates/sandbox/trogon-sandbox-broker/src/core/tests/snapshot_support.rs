use super::*;
use trogon_sandbox_contract::{DriveName, SnapshotTagName};

pub(super) fn tree_digest(n: usize) -> anyhow::Result<Digest> {
    Ok(Digest::from_str(&format!("sha256:{:064x}", n))?)
}

pub(super) fn tree_record(tenant: &str, n: usize, parent: Option<usize>) -> anyhow::Result<SnapshotKvRecord> {
    Ok(SnapshotKvRecord {
        lease_id: LeaseId::new(format!("lease-{n}")),
        tenant: TenantId::new(tenant)?,
        digest: tree_digest(n)?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        registry: "registry.example.com".to_string(),
        repository: "snapshots/test-tenant".to_string(),
        parent: parent.map(tree_digest).transpose()?,
        created_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(n as u64)),
        env_digest: None,
    })
}

pub(super) async fn tree_of(
    records: Vec<SnapshotKvRecord>,
    tenant: &str,
    start: usize,
) -> anyhow::Result<Vec<SnapshotTreeNode>> {
    let kv = Arc::new(InMemorySnapshotKvReader::new());
    for record in records {
        kv.put(record);
    }
    let core = make_core(mock_provider_response()?)?.with_snapshot_kv_reader(kv);
    Ok(core
        .handle_snapshot_tree(SnapshotTreeRequest {
            tenant: TenantId::new(tenant)?,
            digest: tree_digest(start)?,
        })
        .await
        .nodes)
}

pub(super) fn make_update_lease_tags_request(lease_id: &LeaseId, tags: SandboxTags) -> UpdateLeaseTagsRequest {
    UpdateLeaseTagsRequest {
        lease_id: lease_id.clone(),
        tags,
    }
}

pub(super) fn make_extend_lease_timeout_request(
    lease_id: &LeaseId,
    additional_seconds: u32,
) -> ExtendLeaseTimeoutRequest {
    ExtendLeaseTimeoutRequest {
        lease_id: lease_id.clone(),
        additional_seconds,
    }
}

pub(super) fn make_drive_id(tenant: &TenantId, name: &str) -> anyhow::Result<DriveId> {
    Ok(DriveId {
        tenant: tenant.clone(),
        name: DriveName::new(name)?,
    })
}

pub(super) fn snapshot_tag_of(tenant: &str, name: &str) -> anyhow::Result<SnapshotTag> {
    Ok(SnapshotTag {
        tenant: TenantId::new(tenant)?,
        name: SnapshotTagName::new(name)?,
    })
}

pub(super) fn tagged(mut record: LeaseRecord, pairs: &[(&str, &str)]) -> anyhow::Result<LeaseRecord> {
    record.tags = SandboxTags::new(tag_filter(pairs))?;
    Ok(record)
}

pub(super) fn tag_filter(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Grants every lease and records each release it is asked to publish.
pub(super) struct ReleaseRecordingBus {
    pub(super) released: std::sync::Mutex<Vec<LeaseReleased>>,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for ReleaseRecordingBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(mock_provider_response()?)
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        if subject.starts_with("sandbox.lease.released.") {
            locked(&self.released)?.push(serde_json::from_slice(&payload)?);
        }
        Ok(())
    }
}

/// Hides cached grants from `get`, so two requests with one key both
/// dispatch, the way two brokers racing on the same key do.
pub(super) struct RacingIdempotencyStore(pub(super) InMemoryIdempotencyStore);

#[async_trait::async_trait]
impl crate::idempotency::IdempotencyStore for RacingIdempotencyStore {
    async fn get(
        &self,
        _: &TenantId,
        _: &str,
    ) -> Result<Option<LeaseGranted>, crate::idempotency::IdempotencyStoreError> {
        Ok(None)
    }
    async fn put_if_absent(
        &self,
        tenant: &TenantId,
        key: &str,
        response: &LeaseGranted,
    ) -> Result<Option<LeaseGranted>, crate::idempotency::IdempotencyStoreError> {
        self.0.put_if_absent(tenant, key, response).await
    }
    async fn get_attempt(
        &self,
        tenant: &TenantId,
        key: &trogon_sandbox_contract::LeaseRequestKey,
    ) -> Result<Option<LeaseRecord>, crate::idempotency::IdempotencyStoreError> {
        self.0.get_attempt(tenant, key).await
    }
    async fn begin_attempt(
        &self,
        tenant: &TenantId,
        key: &trogon_sandbox_contract::LeaseRequestKey,
        record: &LeaseRecord,
    ) -> Result<bool, crate::idempotency::IdempotencyStoreError> {
        self.0.begin_attempt(tenant, key, record).await
    }
    async fn replace_attempt(
        &self,
        tenant: &TenantId,
        key: &trogon_sandbox_contract::LeaseRequestKey,
        expected: &LeaseRecord,
        replacement: &LeaseRecord,
    ) -> Result<bool, crate::idempotency::IdempotencyStoreError> {
        self.0.replace_attempt(tenant, key, expected, replacement).await
    }
    async fn forget_attempt(
        &self,
        tenant: &TenantId,
        key: &trogon_sandbox_contract::LeaseRequestKey,
        lease_id: &LeaseId,
    ) -> Result<bool, crate::idempotency::IdempotencyStoreError> {
        self.0.forget_attempt(tenant, key, lease_id).await
    }
}

// --- image allow-list integration tests ---
