use std::future::Future;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_observability::{HYPERLIGHT_WORKER, bootstrap};
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_connect, register_fence_file};
use trogon_std::env::SystemEnv;
use uuid::Uuid;

use crate::{HyperlightRuntime, HyperlightWorker, HyperlightWorkerProvider, Hypervisor, LaunchError, WorkerArgs};

/// Runs the worker with telemetry until it stops, and turns the outcome into an exit code.
pub async fn serve<H: Hypervisor>(args: WorkerArgs, hypervisor: H) -> ExitCode {
    report(run(args, hypervisor).await)
}

/// The exit code for how the worker ended; a failure is printed to standard error.
pub fn report(result: Result<(), LaunchError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run<H: Hypervisor>(args: WorkerArgs, hypervisor: H) -> Result<(), LaunchError> {
    register_fence_file(args.dispatch_fence_file.as_deref());
    let _telemetry = bootstrap(
        "trogon-sandbox-hyperlight-worker",
        HYPERLIGHT_WORKER,
        &args.observability,
    )?;
    launch(args, hypervisor).await
}

/// Runs the worker described by `args` until the process is interrupted.
pub async fn launch<H: Hypervisor>(args: WorkerArgs, hypervisor: H) -> Result<(), LaunchError> {
    launch_with(args, hypervisor, tokio::signal::ctrl_c()).await
}

/// Runs the worker described by `args` until `interrupt` completes.
pub async fn launch_with<H, I>(args: WorkerArgs, hypervisor: H, interrupt: I) -> Result<(), LaunchError>
where
    H: Hypervisor,
    I: Future,
{
    let mode = AuthorityMode::from_env(&SystemEnv)?;
    let stop = CancellationToken::new();
    let worker = launch_until(args, mode, hypervisor, stop.clone());
    tokio::pin!(worker);
    tokio::select! {
        result = &mut worker => result,
        _ = interrupt => {
            stop.cancel();
            worker.await
        }
    }
}

/// Runs the worker described by `args` until `stop` fires.
pub async fn launch_until<H: Hypervisor>(
    args: WorkerArgs,
    mode: AuthorityMode,
    hypervisor: H,
    stop: CancellationToken,
) -> Result<(), LaunchError> {
    let insecure_registries = args
        .insecure_registries
        .iter()
        .filter(|host| !host.is_empty())
        .map(RegistryHost::new)
        .collect::<Result<Vec<_>, _>>()?;
    let oci_client = Arc::new(OciClient::new(OciClientConfig {
        insecure_registries,
        ..Default::default()
    }));
    let runtime = HyperlightRuntime::new(
        hypervisor,
        args.component_root,
        args.max_warm_sandboxes,
        oci_client,
        args.snapshot_registry,
    )?;
    let provider = Arc::new(HyperlightWorkerProvider::new(runtime, args.max_concurrent_leases));
    let worker_id = WorkerId::new(Uuid::new_v4().to_string())?;
    let client = nats_connect::connect(&args.nats_url).await?;
    tracing::info!(
        max_concurrent_leases = args.max_concurrent_leases,
        max_warm_sandboxes = args.max_warm_sandboxes,
        "trogon-sandbox-hyperlight-worker starting"
    );
    HyperlightWorker::new(client, provider, AuthorityConfig::new(mode), worker_id)
        .run(stop)
        .await?;
    Ok(())
}
