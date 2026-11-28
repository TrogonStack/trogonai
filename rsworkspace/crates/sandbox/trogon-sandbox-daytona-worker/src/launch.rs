use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerDispatchProtocol, WorkerId};
use trogon_sandbox_protocol::{
    AttemptLedger, DEFAULT_HEARTBEAT_INTERVAL, nats_connect, register_fence_file, spawn_heartbeat_loop,
};

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
    let worker_id = match &args.dispatch_ledger_file {
        Some(path) => {
            let ledger = Arc::new(AttemptLedger::open(path, ProviderKind::Daytona, None)?);
            let worker_id = ledger.worker_id().clone();
            daytona.configure_dispatch_ledger(ledger)?;
            worker_id
        }
        None => WorkerId::new(uuid::Uuid::new_v4().simple().to_string())?,
    };
    let dispatch_protocol = if daytona.dispatch_ledger().is_some() {
        WorkerDispatchProtocol::TargetedV1
    } else {
        WorkerDispatchProtocol::Legacy
    };
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        shutdown.await;
        stop.cancel();
    });
    let health = args.observability.health();
    health.started_with_nats(&nats);
    health.shutdown_when(cancel.clone().cancelled_owned());
    let (heartbeat_provider, heartbeat_id) = (Arc::clone(&daytona), worker_id.clone());
    let max_concurrent_leases = args.max_concurrent_leases;
    let regions = args.regions.clone();
    // Heartbeat only once every subject is served, so the broker never targets a worker that cannot yet listen.
    let (heartbeat_nats, ready_provider) = (nats.clone(), Arc::clone(&daytona));
    let (beat_worker, beat_cancel) = (worker_id.clone(), cancel.clone());
    tokio::spawn(async move {
        ready_provider.wait_ready().await;
        spawn_heartbeat_loop(
            heartbeat_nats,
            ProviderKind::Daytona,
            beat_worker,
            DEFAULT_HEARTBEAT_INTERVAL,
            beat_cancel,
            move || {
                let (provider, worker_id, regions) =
                    (Arc::clone(&heartbeat_provider), heartbeat_id.clone(), regions.clone());
                async move {
                    let current = u32::try_from(provider.active_lease_count().await).unwrap_or(u32::MAX);
                    build_heartbeat(&worker_id, max_concurrent_leases, current, &regions, dispatch_protocol)
                }
            },
        );
    });
    info!("trogon-sandbox-daytona-worker starting");
    run(nats, js, daytona, &worker_id, cancel).await
}
