//! Per-tenant records kept in NATS KV: image allow-list, host-mount allow-list, native
//! passthrough, and request limits.

use async_nats::jetstream::kv;
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_broker::host_mount_policy::{NatsKvHostMountAllowList, TenantHostMountPolicy};
use trogon_sandbox_broker::image_policy::{NatsKvImageAllowList, TenantImagePolicy};
use trogon_sandbox_broker::limits::{NatsKvTenantLimits, PerTagQuota, TenantLimit};
use trogon_sandbox_broker::native_policy::{NatsKvNativePassthroughStore, TenantNativePassthroughPolicy};
use trogon_sandbox_broker::tenant_record;
use trogon_sandbox_contract::TenantId;

use crate::{AdminError, Console};

/// An image pattern is an exact reference or a prefix ending in one trailing `*`.
pub fn validate_pattern(pattern: &str) -> Result<(), AdminError> {
    if pattern.is_empty() {
        return Err(AdminError::EmptyPattern);
    }
    let prefix_to_check = pattern.strip_suffix('*').unwrap_or(pattern);
    if prefix_to_check.contains('*') {
        return Err(AdminError::MisplacedWildcard {
            pattern: pattern.to_string(),
        });
    }
    Ok(())
}

/// A host-mount prefix is an absolute path.
pub fn validate_host_mount_prefix(prefix: &str) -> Result<(), AdminError> {
    if prefix.is_empty() {
        return Err(AdminError::EmptyHostMountPrefix);
    }
    if !prefix.starts_with('/') {
        return Err(AdminError::RelativeHostMountPrefix {
            prefix: prefix.to_string(),
        });
    }
    Ok(())
}

async fn put_record<T: Serialize>(
    kv: &kv::Store,
    tenant: &TenantId,
    record: &T,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let bytes = Bytes::from(serde_json::to_vec(record).map_err(AdminError::Encode)?);
    tenant_record::put(kv, tenant, bytes)
        .await
        .map_err(AdminError::step("write tenant record"))?;
    console.out_json(record)
}

async fn show_record<T: DeserializeOwned + Serialize>(
    kv: &kv::Store,
    tenant: &TenantId,
    missing: &str,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    match tenant_record::get_parsed::<T>(kv, tenant)
        .await
        .map_err(AdminError::step("read tenant record"))?
    {
        Some(record) => console.out_json(&record),
        None => console.out(format_args!("no {missing} found for tenant '{tenant}'")),
    }
}

async fn clear_record(kv: &kv::Store, tenant: &TenantId, console: &mut Console<'_>) -> Result<(), AdminError> {
    let removed = tenant_record::clear(kv, tenant)
        .await
        .map_err(AdminError::step("clear tenant record"))?;
    console.out(if removed { "cleared" } else { "no such key" })
}

async fn existing<T: DeserializeOwned>(kv: &kv::Store, tenant: &TenantId) -> Result<Option<T>, AdminError> {
    tenant_record::get_parsed::<T>(kv, tenant)
        .await
        .map_err(AdminError::step("read tenant record"))
}

pub async fn image_policy_set(
    store: &NatsKvImageAllowList,
    tenant: &TenantId,
    new_patterns: Vec<String>,
    replace: bool,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let mut patterns = if replace {
        Vec::new()
    } else {
        existing::<TenantImagePolicy>(&store.kv, tenant)
            .await?
            .map(|policy| policy.patterns)
            .unwrap_or_default()
    };
    patterns.extend(new_patterns);
    put_record(&store.kv, tenant, &TenantImagePolicy::from_patterns(patterns), console).await
}

pub async fn image_policy_show(
    store: &NatsKvImageAllowList,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    show_record::<TenantImagePolicy>(&store.kv, tenant, "policy", console).await
}

pub async fn image_policy_clear(
    store: &NatsKvImageAllowList,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    clear_record(&store.kv, tenant, console).await
}

pub async fn host_mount_set(
    store: &NatsKvHostMountAllowList,
    tenant: &TenantId,
    new_prefixes: Vec<String>,
    replace: bool,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let mut path_prefixes = if replace {
        Vec::new()
    } else {
        existing::<TenantHostMountPolicy>(&store.kv, tenant)
            .await?
            .map(|policy| policy.path_prefixes)
            .unwrap_or_default()
    };
    path_prefixes.extend(new_prefixes);
    put_record(&store.kv, tenant, &TenantHostMountPolicy { path_prefixes }, console).await
}

pub async fn host_mount_show(
    store: &NatsKvHostMountAllowList,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    show_record::<TenantHostMountPolicy>(&store.kv, tenant, "policy", console).await
}

pub async fn host_mount_clear(
    store: &NatsKvHostMountAllowList,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    clear_record(&store.kv, tenant, console).await
}

pub async fn native_passthrough_set(
    store: &NatsKvNativePassthroughStore,
    tenant: &TenantId,
    allowed: bool,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    put_record(&store.kv, tenant, &TenantNativePassthroughPolicy { allowed }, console).await
}

pub async fn native_passthrough_show(
    store: &NatsKvNativePassthroughStore,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    match store
        .policy_for(tenant)
        .await
        .map_err(AdminError::step("read tenant record"))?
    {
        Some(policy) => console.out_json(&policy),
        None => console.out(format_args!("no policy found for tenant '{tenant}'")),
    }
}

pub async fn native_passthrough_clear(
    store: &NatsKvNativePassthroughStore,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    clear_record(&store.kv, tenant, console).await
}

pub async fn tenant_limit_set(
    store: &NatsKvTenantLimits,
    tenant: &TenantId,
    max_cpu_seconds: u32,
    max_memory_mib: u32,
    per_tag_quota: Option<PerTagQuota>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let limit = TenantLimit {
        max_cpu_seconds_per_request: max_cpu_seconds,
        max_memory_mib_per_request: max_memory_mib,
        per_tag_quota,
    };
    put_record(&store.kv, tenant, &limit, console).await
}

pub async fn tenant_limit_show(
    store: &NatsKvTenantLimits,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    show_record::<TenantLimit>(&store.kv, tenant, "limit", console).await
}

pub async fn tenant_limit_clear(
    store: &NatsKvTenantLimits,
    tenant: &TenantId,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    clear_record(&store.kv, tenant, console).await
}
