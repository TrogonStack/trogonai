use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerDispatchProtocol, WorkerId};
use trogon_sandbox_protocol::{
    AttemptLedger, AuthorityConfig, AuthorityMode, DEFAULT_HEARTBEAT_INTERVAL, nats_connect, register_fence_file,
    spawn_heartbeat_loop,
};
use trogon_std::env::ReadEnv;

use crate::heartbeat::build_heartbeat;
use crate::terminal::{E2bGuestShell, E2bTerminals, run_terminal_handler};
use crate::ulid::Ulid;
use crate::worker::log_exit;
use crate::{E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd, LaunchError, WorkerArgs, run};

/// Starts the worker `args` describe and serves until `shutdown` completes.
pub async fn launch(
    args: &WorkerArgs,
    env: &impl ReadEnv,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), LaunchError> {
    register_fence_file(args.dispatch_fence_file.as_deref());
    let authority = AuthorityConfig::new(AuthorityMode::from_env(env)?);
    let mut config = E2bConfig::new(args.api_url.clone(), &args.domain, &args.api_key)?;
    if let Some(url) = &args.envd_url {
        config = config.with_envd_url(url.clone())?;
    }
    let timeout = Duration::from_secs(args.http_timeout_secs);
    let api = E2bApi::new(config, timeout)?;
    let envd = Envd::new(timeout)?;
    let nats = nats_connect::connect(&args.nats_url).await?;
    let js = jetstream::new(nats.clone());
    let exec = Arc::new(E2bExec::new(envd.clone()).with_evidence(js.clone()));
    let e2b = Arc::new(E2bWorkerProvider::new(api, args.template.clone(), exec));
    let worker_id = match &args.dispatch_ledger_file {
        Some(path) => {
            let ledger = Arc::new(AttemptLedger::open_for_adoption(path, ProviderKind::E2b, None)?);
            let worker_id = ledger.worker_id().clone();
            e2b.configure_dispatch_ledger(ledger)?;
            worker_id
        }
        None => WorkerId::new(Ulid::generate().to_string())?,
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
    spawn_heartbeats(
        nats.clone(),
        &worker_id,
        args.max_concurrent_leases,
        args.regions.clone(),
        Arc::clone(&e2b),
        cancel.clone(),
    );
    let shell = Arc::new(E2bGuestShell::new(Arc::clone(&e2b), envd));
    let terminals = Arc::new(E2bTerminals::new(shell, nats.clone()));
    let (terminal_client, terminal_hub) = (nats.clone(), Arc::clone(&terminals));
    let terminal_stop = cancel.clone();
    let terminal = tokio::spawn(async move {
        let result = tokio::select! {
            () = terminal_stop.cancelled() => Ok(()),
            result = run_terminal_handler(terminal_client, terminal_hub) => result,
        };
        log_exit("terminal handler", result);
    });
    info!("trogon-sandbox-e2b-worker starting");
    let served = run(nats, js, e2b, &worker_id, cancel.clone(), terminals, authority).await;
    cancel.cancel();
    log_exit("terminal handler task", terminal.await);
    served?;
    Ok(())
}

/// Heartbeats once the provider has finished starting, so the broker never sees a worker that cannot yet serve.
fn spawn_heartbeats(
    client: async_nats::Client,
    worker_id: &WorkerId,
    max_concurrent_leases: u32,
    regions: Vec<String>,
    provider: Arc<E2bWorkerProvider>,
    cancel: CancellationToken,
) {
    let worker_id = worker_id.clone();
    tokio::spawn(async move {
        provider.wait_ready().await;
        let beat_id = worker_id.clone();
        spawn_heartbeat_loop(
            client,
            ProviderKind::E2b,
            worker_id,
            DEFAULT_HEARTBEAT_INTERVAL,
            cancel,
            move || {
                let (worker_id, regions, provider) = (beat_id.clone(), regions.clone(), Arc::clone(&provider));
                async move {
                    let current = u32::try_from(provider.active_lease_count().await).unwrap_or(u32::MAX);
                    let protocol = if provider.dispatch_ledger().is_some() {
                        WorkerDispatchProtocol::TargetedV1
                    } else {
                        WorkerDispatchProtocol::Legacy
                    };
                    build_heartbeat(&worker_id, max_concurrent_leases, current, &regions, protocol)
                }
            },
        );
    });
}

#[cfg(test)]
mod tests;
