use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;

use super::serve_error::ServeError;
use crate::cli::Cli;
use crate::worker_fence::{FENCE_REFRESH_INTERVAL, NatsKvWorkerFenceStore, spawn_fence_refresh};
use crate::worker_registry::{NatsWorkerRegistry, WorkerRegistry};

/// Opens the heartbeat backed worker registry and keeps its fence view fresh until `cancel` fires.
pub(super) async fn open(
    cli: &Cli,
    nats: &async_nats::Client,
    jetstream: &jetstream::Context,
    cancel: CancellationToken,
) -> Result<Arc<dyn WorkerRegistry>, ServeError> {
    let registry = NatsWorkerRegistry::new(
        nats.clone(),
        Duration::from_secs(cli.worker_heartbeat_ttl_secs),
        cancel.clone(),
    )
    .await
    .map_err(ServeError::opening("worker registry"))?;
    let fences = NatsKvWorkerFenceStore::new(jetstream.clone())
        .await
        .map_err(ServeError::opening("worker fence store"))?;
    spawn_fence_refresh(Arc::new(fences), registry.fences(), FENCE_REFRESH_INTERVAL, cancel);
    Ok(registry)
}
