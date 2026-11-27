use std::sync::Arc;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, Provider, spawn_heartbeat_loop};

use crate::{ServeError, V8WasmProvider, WorkerArgs, build_heartbeat, run_worker};

/// Connects to NATS and serves leases as a new worker until `cancel` fires.
pub async fn serve(args: &WorkerArgs, cancel: CancellationToken) -> Result<(), ServeError> {
    let nats = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| ServeError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let v8 = Arc::new(V8WasmProvider::new(jetstream::new(nats.clone())));
    let provider: Arc<dyn Provider> = v8.clone();
    let worker = WorkerId::new(uuid::Uuid::new_v4().to_string())?;
    let max_concurrent_leases = args.max_concurrent_leases;

    let (id, heartbeat_host) = (worker.clone(), v8.clone());
    spawn_heartbeat_loop(
        nats.clone(),
        ProviderKind::V8Wasm,
        worker.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let (id, host) = (id.clone(), heartbeat_host.clone());
            async move {
                let mut heartbeat = build_heartbeat(&id, max_concurrent_leases);
                heartbeat.capacity.current_leases = u32::try_from(host.active_lease_count().await).unwrap_or(u32::MAX);
                heartbeat
            }
        },
    );
    info!(max_concurrent_leases, "worker starting");
    Ok(run_worker(nats, provider, v8, worker, cancel).await?)
}
