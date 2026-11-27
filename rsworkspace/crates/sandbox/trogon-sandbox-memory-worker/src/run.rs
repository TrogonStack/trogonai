use std::future::Future;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, register_fence_file, spawn_heartbeat_loop};

use crate::{MemoryWorker, MemoryWorkerArgs, MemoryWorkerConfig, RunError};

/// Serves the provider subjects and heartbeats until `shutdown` completes, then closes every lease.
pub async fn run(args: MemoryWorkerArgs, shutdown: impl Future<Output = ()>) -> Result<(), RunError> {
    register_fence_file(args.dispatch_fence_file.as_deref());
    let client = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| RunError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let config = MemoryWorkerConfig::default();
    let kind = config.kind;
    let worker = MemoryWorker::start(client.clone(), config).await?;
    let make_heartbeat = worker.heartbeat_source(args.max_concurrent_leases, args.regions);
    let heartbeat_cancel = CancellationToken::new();
    spawn_heartbeat_loop(
        client,
        kind,
        worker.worker_id().clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        heartbeat_cancel.clone(),
        move || {
            let heartbeat = make_heartbeat();
            async move { heartbeat }
        },
    );
    shutdown.await;
    heartbeat_cancel.cancel();
    worker.shutdown().await;
    Ok(())
}
