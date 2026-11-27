use std::sync::Arc;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop};

use crate::{AcaWorkerProvider, BrokerCredentialIssuer, ServeError, WorkerArgs, build_heartbeat, run_worker};

/// Connects to NATS and serves leases as a new worker until `cancel` fires.
pub async fn serve(args: &WorkerArgs, cancel: CancellationToken) -> Result<(), ServeError> {
    let nats = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| ServeError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let provider = Arc::new(
        AcaWorkerProvider::new(
            jetstream::new(nats.clone()),
            args.provider_config(),
            Arc::new(BrokerCredentialIssuer::new(nats.clone())),
        )
        .with_max_lease_lifetime(args.max_lease_lifetime_seconds),
    );
    let worker = WorkerId::new(uuid::Uuid::new_v4().to_string())?;
    let max_concurrent_leases = args.max_concurrent_leases;
    let region = args.region.clone();

    let (id, heartbeat_host) = (worker.clone(), provider.clone());
    spawn_heartbeat_loop(
        nats.clone(),
        ProviderKind::AzureContainerApps,
        worker.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let (id, host, region) = (id.clone(), heartbeat_host.clone(), region.clone());
            async move {
                let current = u32::try_from(host.active_lease_count().await).unwrap_or(u32::MAX);
                build_heartbeat(&id, max_concurrent_leases, current, region.as_deref())
            }
        },
    );
    info!(max_concurrent_leases, "worker starting");
    Ok(run_worker(nats, provider, worker, cancel).await?)
}
