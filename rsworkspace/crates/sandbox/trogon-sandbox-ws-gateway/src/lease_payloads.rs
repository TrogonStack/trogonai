use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{
    LeaseForkRequest, LeaseId, NativeSnapshotAcceptance, ReapLeasesRequest, ReapMode, SandboxTags, SuspendRequestLevel,
    TenantId,
};

use crate::PayloadError;
use crate::from_payload::from_payload;

#[derive(Deserialize)]
pub(crate) struct LeaseIdPayload {
    pub lease_id: LeaseId,
}

#[derive(Deserialize)]
struct TagFilterPayload {
    #[serde(default)]
    tags: Option<BTreeMap<String, String>>,
    #[serde(default)]
    tag_key: Option<String>,
    #[serde(default)]
    tag_value: Option<String>,
}

impl TagFilterPayload {
    fn into_filter(self) -> BTreeMap<String, String> {
        let mut filter = self.tags.unwrap_or_default();
        if let (Some(key), Some(value)) = (self.tag_key, self.tag_value) {
            filter.insert(key, value);
        }
        filter
    }
}

#[derive(Deserialize)]
struct ReapPayload {
    #[serde(default)]
    older_than_seconds: Option<u32>,
    #[serde(default)]
    idle_for_seconds: Option<u32>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Deserialize)]
struct UpdateTagsPayload {
    lease_id: LeaseId,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct PausePayload {
    lease_id: LeaseId,
    #[serde(default)]
    max_paused_seconds: Option<u32>,
}

#[derive(Deserialize)]
struct SuspendPayload {
    lease_id: LeaseId,
    #[serde(default)]
    level: Option<SuspendRequestLevel>,
    #[serde(default)]
    max_suspended_seconds: Option<u32>,
}

#[derive(Deserialize)]
struct ForkPayload {
    source_lease_id: LeaseId,
    ttl_seconds: u32,
    idempotency_key: String,
}

#[derive(Deserialize)]
struct SnapshotLeasePayload {
    lease_id: LeaseId,
    #[serde(default)]
    accept_native: NativeSnapshotAcceptance,
}

#[derive(Deserialize)]
struct SshAccessPayload {
    lease_id: LeaseId,
    #[serde(default)]
    ttl_seconds: Option<u32>,
}

/// The tag filter of `sandbox_list_leases`: an optional `tags` object (every entry must match) plus the older
/// `tag_key` and `tag_value` pair. Empty when neither is present.
pub(crate) fn tag_filter(payload: &Value) -> Result<BTreeMap<String, String>, PayloadError> {
    Ok(from_payload::<TagFilterPayload>(payload)?.into_filter())
}

/// A `sandbox_reap_leases` payload. The tenant is always the session's, never one the payload names.
pub(crate) fn reap_leases(payload: &Value, tenant: &TenantId) -> Result<ReapLeasesRequest, PayloadError> {
    let tags = SandboxTags::new(tag_filter(payload)?)?;
    let reap: ReapPayload = from_payload(payload)?;
    let mode = if reap.dry_run {
        ReapMode::DryRun
    } else {
        ReapMode::Release
    };
    Ok(ReapLeasesRequest::new(
        tenant.clone(),
        tags,
        reap.older_than_seconds,
        reap.idle_for_seconds,
        mode,
    )?)
}

pub(crate) fn update_tags(payload: &Value) -> Result<(LeaseId, SandboxTags), PayloadError> {
    let update: UpdateTagsPayload = from_payload(payload)?;
    Ok((update.lease_id, SandboxTags::new(update.tags)?))
}

pub(crate) fn pause_lease(payload: &Value) -> Result<(LeaseId, Option<u32>), PayloadError> {
    let pause: PausePayload = from_payload(payload)?;
    Ok((pause.lease_id, pause.max_paused_seconds))
}

pub(crate) fn suspend_lease(payload: &Value) -> Result<(LeaseId, SuspendRequestLevel, Option<u32>), PayloadError> {
    let suspend: SuspendPayload = from_payload(payload)?;
    Ok((
        suspend.lease_id,
        suspend.level.unwrap_or(SuspendRequestLevel::Auto),
        suspend.max_suspended_seconds,
    ))
}

pub(crate) fn fork_lease(payload: &Value, tenant: &TenantId) -> Result<LeaseForkRequest, PayloadError> {
    let fork: ForkPayload = from_payload(payload)?;
    Ok(LeaseForkRequest {
        tenant: tenant.clone(),
        source_lease_id: fork.source_lease_id,
        ttl_seconds: fork.ttl_seconds,
        idempotency_key: fork.idempotency_key,
    })
}

pub(crate) fn snapshot_lease(payload: &Value) -> Result<(LeaseId, NativeSnapshotAcceptance), PayloadError> {
    let snapshot: SnapshotLeasePayload = from_payload(payload)?;
    Ok((snapshot.lease_id, snapshot.accept_native))
}

pub(crate) fn ssh_access(payload: &Value) -> Result<(LeaseId, Option<u32>), PayloadError> {
    let ssh: SshAccessPayload = from_payload(payload)?;
    Ok((ssh.lease_id, ssh.ttl_seconds))
}

/// The lease a payload names.
pub(crate) fn lease_id(payload: &Value) -> Result<LeaseId, PayloadError> {
    Ok(from_payload::<LeaseIdPayload>(payload)?.lease_id)
}
