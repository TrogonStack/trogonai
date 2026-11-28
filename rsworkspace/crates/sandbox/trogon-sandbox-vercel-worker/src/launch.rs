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

use crate::heartbeat::build_heartbeat;
use crate::{
    VercelApi, VercelConfig, VercelExec, VercelWorkerError, VercelWorkerProvider, WorkerArgs, check_runtime, run,
};

/// Starts the worker `args` describe and serves until `shutdown` completes.
pub async fn launch(
    args: &WorkerArgs,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), VercelWorkerError> {
    register_fence_file(args.dispatch_fence_file.as_deref());
    check_runtime(&args.runtime)?;
    let max_vcpus = args.max_vcpus.min(args.plan.max_vcpus());
    let nats = nats_connect::connect(&args.nats_url).await?;
    let js = jetstream::new(nats.clone());
    let config = VercelConfig::new(
        args.api_url.clone(),
        args.team_id.clone(),
        args.project_id.clone(),
        args.token.clone(),
    );
    let api = VercelApi::new(config, Duration::from_secs(args.http_timeout_secs))?;
    let exec = Arc::new(VercelExec::new(api.clone()));
    let vercel = Arc::new(VercelWorkerProvider::new(
        api,
        args.runtime.clone(),
        max_vcpus,
        args.plan,
        exec,
    ));
    let worker_id = match &args.dispatch_ledger_file {
        Some(path) => {
            let ledger = Arc::new(AttemptLedger::open(path, ProviderKind::Vercel, None)?);
            let worker_id = ledger.worker_id().clone();
            vercel.configure_dispatch_ledger(ledger)?;
            worker_id
        }
        None => WorkerId::new(uuid::Uuid::new_v4().simple().to_string())?,
    };
    let dispatch_protocol = if vercel.dispatch_ledger().is_some() {
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
    let (regions, plan, heartbeat_provider, heartbeat_id) =
        (args.regions.clone(), args.plan, Arc::clone(&vercel), worker_id.clone());
    let max_concurrent_leases = args.max_concurrent_leases;
    // Heartbeat only once every subject is served, so the broker never targets a worker that cannot yet listen.
    let (heartbeat_nats, ready_provider) = (nats.clone(), Arc::clone(&vercel));
    let (beat_worker, beat_cancel) = (worker_id.clone(), cancel.clone());
    tokio::spawn(async move {
        ready_provider.wait_ready().await;
        spawn_heartbeat_loop(
            heartbeat_nats,
            ProviderKind::Vercel,
            beat_worker,
            DEFAULT_HEARTBEAT_INTERVAL,
            beat_cancel,
            move || {
                let (regions, provider, worker_id) =
                    (regions.clone(), Arc::clone(&heartbeat_provider), heartbeat_id.clone());
                async move {
                    let current = u32::try_from(provider.active_lease_count().await).unwrap_or(u32::MAX);
                    build_heartbeat(
                        &worker_id,
                        max_concurrent_leases,
                        current,
                        &regions,
                        max_vcpus,
                        plan,
                        dispatch_protocol,
                    )
                }
            },
        );
    });
    info!("trogon-sandbox-vercel-worker starting");
    run(nats, js, vercel, &worker_id, cancel).await
}
