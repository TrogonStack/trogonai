use std::num::NonZeroU32;

use serde::Serialize;
use trogon_sandbox_contract::{
    IdlePolicy, LeaseId, LeaseName, LeaseRecord, LeaseStatus, ProviderKind, SandboxImage, SandboxTags, TenantId,
};

use crate::unix_seconds::unix_seconds;

/// One lease as the dashboard lists it.
#[derive(Debug, Serialize)]
pub(crate) struct LeaseRow<'a> {
    pub(crate) lease_id: &'a LeaseId,
    tenant: &'a TenantId,
    provider: ProviderKind,
    image: &'a SandboxImage,
    state: LeaseStatus,
    started_at: u64,
    ttl_seconds: u64,
    tags: &'a SandboxTags,
    idle_timeout_seconds: Option<NonZeroU32>,
    last_activity_at: Option<u64>,
    idle_policy: Option<&'a IdlePolicy>,
    auto_paused_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a LeaseName>,
}

impl<'a> From<&'a LeaseRecord> for LeaseRow<'a> {
    fn from(record: &'a LeaseRecord) -> Self {
        Self {
            lease_id: &record.lease_id,
            tenant: &record.tenant,
            provider: record.provider,
            image: record.spec.image(),
            state: record.status,
            started_at: unix_seconds(record.created_at),
            ttl_seconds: record
                .deadline
                .duration_since(record.created_at)
                .map_or(0, |ttl| ttl.as_secs()),
            tags: &record.tags,
            idle_timeout_seconds: record.spec.idle_timeout_seconds(),
            last_activity_at: record.last_activity_at.map(unix_seconds),
            idle_policy: record.spec.idle_policy(),
            auto_paused_at: record.auto_paused_at.map(unix_seconds),
            name: record.name.as_ref(),
        }
    }
}
