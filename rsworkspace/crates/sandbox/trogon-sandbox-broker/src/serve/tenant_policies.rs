use std::sync::Arc;

use async_nats::jetstream;

use super::serve_error::ServeError;
use crate::cli::{Cli, QuotaBackend};
use crate::host_mount_policy::{HostMountAllowList, InMemoryHostMountAllowList, NatsKvHostMountAllowList};
use crate::image_policy::{ImageAllowList, InMemoryImageAllowList, NatsKvImageAllowList};
use crate::limits::{InMemoryTenantLimits, NatsKvTenantLimits, TenantLimits};
use crate::native_policy::{InMemoryNativePassthroughStore, NativePassthroughPolicy, NatsKvNativePassthroughStore};

/// Per-tenant rules the broker checks before granting a lease. They follow the quota backend.
pub(super) struct TenantPolicies {
    pub(super) images: Arc<dyn ImageAllowList>,
    pub(super) host_mounts: Arc<dyn HostMountAllowList>,
    pub(super) native_passthrough: Arc<dyn NativePassthroughPolicy>,
    pub(super) limits: Arc<dyn TenantLimits>,
}

impl TenantPolicies {
    pub(super) async fn open(cli: &Cli, jetstream: &jetstream::Context) -> Result<Self, ServeError> {
        match cli.quota_backend {
            QuotaBackend::NatsKv => Self::open_shared(cli, jetstream).await,
            QuotaBackend::InMemory => Ok(Self {
                images: Arc::new(InMemoryImageAllowList::new()),
                host_mounts: Arc::new(InMemoryHostMountAllowList::new()),
                native_passthrough: Arc::new(InMemoryNativePassthroughStore::new()),
                limits: Arc::new(InMemoryTenantLimits::new()),
            }),
        }
    }

    async fn open_shared(cli: &Cli, jetstream: &jetstream::Context) -> Result<Self, ServeError> {
        Ok(Self {
            images: Arc::new(
                NatsKvImageAllowList::new(jetstream.clone(), &cli.image_allowlist_bucket)
                    .await
                    .map_err(ServeError::opening("image allow-list store"))?,
            ),
            host_mounts: Arc::new(
                NatsKvHostMountAllowList::new(jetstream.clone(), &cli.host_mount_allowlist_bucket)
                    .await
                    .map_err(ServeError::opening("host-mount allow-list store"))?,
            ),
            native_passthrough: Arc::new(
                NatsKvNativePassthroughStore::new(jetstream.clone(), &cli.native_passthrough_bucket)
                    .await
                    .map_err(ServeError::opening("native passthrough store"))?,
            ),
            limits: Arc::new(
                NatsKvTenantLimits::new(jetstream.clone(), &cli.tenant_limits_bucket)
                    .await
                    .map_err(ServeError::opening("tenant limits store"))?,
            ),
        })
    }
}
