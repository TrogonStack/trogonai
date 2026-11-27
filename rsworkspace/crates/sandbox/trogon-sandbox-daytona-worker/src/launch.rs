use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, nats_connect, register_fence_file, spawn_heartbeat_loop};

use crate::constants::STATE_POLL_INTERVAL;
use crate::heartbeat::build_heartbeat;
use crate::{
    DaytonaApi, DaytonaConfig, DaytonaExec, DaytonaWorkerError, DaytonaWorkerProvider, OrdinaryCommandRuntime,
    SnapshotCustody, SnapshotWait, Wait, WorkerArgs, run,
};

/// Starts the worker `args` describe and serves until `shutdown` completes.
pub async fn launch(
    args: &WorkerArgs,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), DaytonaWorkerError> {
    register_fence_file(args.dispatch_fence_file.as_deref());
    let nats = nats_connect::connect(&args.nats_url).await?;
    let js = jetstream::new(nats.clone());
    let config = DaytonaConfig::new(args.api_url.clone(), args.api_key.clone(), args.organization_id.clone());
    let api = DaytonaApi::new(config, Duration::from_secs(args.http_timeout_secs))?;
    let mut exec = DaytonaExec::new(api.clone());
    if let (Some(path), Some(pin)) = (&args.command_supervisor, &args.command_supervisor_sha256) {
        exec = exec.with_runtime(OrdinaryCommandRuntime::load(path, pin)?);
    }
    let exec = Arc::new(exec);
    let custody = SnapshotCustody::new(js.clone(), &api);
    let daytona = Arc::new(
        DaytonaWorkerProvider::new(api, args.snapshot.clone(), exec)
            .with_lifecycle(js.clone())
            .with_wait(Wait {
                timeout: Duration::from_secs(args.start_timeout_secs),
                interval: STATE_POLL_INTERVAL,
            })
            .with_snapshot_custody(custody)
            .with_snapshot_wait(SnapshotWait {
                poll_interval: Duration::from_secs(args.snapshot_poll_interval_secs),
                timeout: Duration::from_secs(args.snapshot_timeout_secs),
            }),
    );
    let worker_id = WorkerId::new(uuid::Uuid::new_v4().simple().to_string())?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        shutdown.await;
        stop.cancel();
    });
    let (heartbeat_provider, heartbeat_id) = (Arc::clone(&daytona), worker_id.clone());
    let max_concurrent_leases = args.max_concurrent_leases;
    let regions = args.regions.clone();
    spawn_heartbeat_loop(
        nats.clone(),
        ProviderKind::Daytona,
        worker_id.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let (provider, worker_id, regions) =
                (Arc::clone(&heartbeat_provider), heartbeat_id.clone(), regions.clone());
            async move {
                let current = u32::try_from(provider.active_lease_count().await).unwrap_or(u32::MAX);
                build_heartbeat(&worker_id, max_concurrent_leases, current, &regions)
            }
        },
    );
    info!("trogon-sandbox-daytona-worker starting");
    run(nats, js, daytona, &worker_id, cancel).await
}
