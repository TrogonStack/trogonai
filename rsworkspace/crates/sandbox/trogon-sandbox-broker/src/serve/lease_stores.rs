use std::sync::Arc;

use async_nats::jetstream;
use tracing::warn;

use super::constants::LAUNCHES_BUCKET_SUFFIX;
use super::serve_error::ServeError;
use crate::active_lease::{ActiveLeaseStore, NatsKvActiveLeaseStore};
use crate::cli::{Cli, IdempotencyBackend, QuotaBackend};
use crate::command_launch::{CommandLaunchStore, InMemoryCommandLaunchStore, NatsKvCommandLaunchStore};
use crate::idempotency::{IdempotencyStore, InMemoryIdempotencyStore, NatsKvIdempotencyStore};
use crate::quota::{InMemoryQuotaStore, NatsKvQuotaStore, QuotaStore, SnapshotQuota};

/// The stores that track what each lease costs and which leases are live.
pub(super) struct LeaseStores {
    pub(super) quota: Arc<dyn QuotaStore>,
    pub(super) idempotency: Arc<dyn IdempotencyStore>,
    pub(super) command_launches: Arc<dyn CommandLaunchStore>,
    pub(super) active_leases: Arc<dyn ActiveLeaseStore>,
}

impl LeaseStores {
    pub(super) async fn open(cli: &Cli, jetstream: &jetstream::Context) -> Result<Self, ServeError> {
        Ok(Self {
            quota: open_quota(cli, jetstream).await?,
            idempotency: open_idempotency(cli, jetstream).await?,
            command_launches: open_command_launches(cli, jetstream).await?,
            active_leases: Arc::new(
                NatsKvActiveLeaseStore::new(jetstream.clone(), &cli.active_lease_bucket)
                    .await
                    .map_err(ServeError::opening("active lease store"))?,
            ),
        })
    }
}

async fn open_quota(cli: &Cli, jetstream: &jetstream::Context) -> Result<Arc<dyn QuotaStore>, ServeError> {
    let defaults = SnapshotQuota::new(cli.default_snapshot_count_cap, cli.default_snapshot_bytes_cap);
    Ok(match cli.quota_backend {
        QuotaBackend::NatsKv => Arc::new(
            NatsKvQuotaStore::new(jetstream.clone(), &cli.quota_bucket)
                .await
                .map_err(ServeError::opening("quota store"))?
                .with_snapshot_defaults(defaults),
        ),
        QuotaBackend::InMemory => {
            warn!("quota backend is in-memory; spend will be lost on restart");
            Arc::new(InMemoryQuotaStore::new().with_snapshot_defaults(defaults))
        }
    })
}

async fn open_idempotency(cli: &Cli, jetstream: &jetstream::Context) -> Result<Arc<dyn IdempotencyStore>, ServeError> {
    Ok(match cli.idempotency_backend {
        IdempotencyBackend::NatsKv => Arc::new(
            NatsKvIdempotencyStore::new(jetstream.clone(), &cli.idempotency_bucket)
                .await
                .map_err(ServeError::opening("idempotency store"))?,
        ),
        IdempotencyBackend::InMemory => {
            warn!("idempotency backend is in-memory; dedup will be lost on restart");
            Arc::new(InMemoryIdempotencyStore::new())
        }
    })
}

async fn open_command_launches(
    cli: &Cli,
    jetstream: &jetstream::Context,
) -> Result<Arc<dyn CommandLaunchStore>, ServeError> {
    Ok(match cli.idempotency_backend {
        IdempotencyBackend::NatsKv => Arc::new(
            NatsKvCommandLaunchStore::new(
                jetstream.clone(),
                &format!("{}{LAUNCHES_BUCKET_SUFFIX}", cli.idempotency_bucket),
                cli.jetstream_replicas,
            )
            .await
            .map_err(ServeError::opening("command launch store"))?,
        ),
        IdempotencyBackend::InMemory => Arc::new(InMemoryCommandLaunchStore::new()),
    })
}
