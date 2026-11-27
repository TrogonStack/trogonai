use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, nats_connect, register_fence_file, spawn_heartbeat_loop};

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
    let worker_id = WorkerId::new(uuid::Uuid::new_v4().simple().to_string())?;
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        shutdown.await;
        stop.cancel();
    });
    let (regions, plan, heartbeat_provider, heartbeat_id) =
        (args.regions.clone(), args.plan, Arc::clone(&vercel), worker_id.clone());
    let max_concurrent_leases = args.max_concurrent_leases;
    spawn_heartbeat_loop(
        nats.clone(),
        ProviderKind::Vercel,
        worker_id.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let (regions, provider, worker_id) =
                (regions.clone(), Arc::clone(&heartbeat_provider), heartbeat_id.clone());
            async move {
                let current = u32::try_from(provider.active_lease_count().await).unwrap_or(u32::MAX);
                build_heartbeat(&worker_id, max_concurrent_leases, current, &regions, max_vcpus, plan)
            }
        },
    );
    info!("trogon-sandbox-vercel-worker starting");
    run(nats, js, vercel, &worker_id, cancel).await
}
