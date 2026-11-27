use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use async_nats::jetstream;
use clap::Parser;
use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_subprocess_worker::heartbeat::spawn_heartbeat_task;
use trogon_sandbox_subprocess_worker::{WindowsWorker, WindowsWorkerConfig};

use crate::worker;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox subprocess worker for Windows: serves sandbox lease requests in per-lease AppContainers, with the network denied unless the network fence is installed"
)]
pub struct Args {
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(long, env = "SANDBOX_AGENT_BINARY")]
    agent_binary: PathBuf,

    #[arg(long = "agent-arg", allow_hyphen_values = true)]
    agent_args: Vec<String>,

    #[arg(long, env = "SANDBOX_WORKSPACE_ROOT")]
    workspace_root: PathBuf,

    /// The egress proxy a lease with a domain allowlist is fenced to. Without
    /// it, or without the network fence installed, the network stays denied.
    #[arg(long, env = "SANDBOX_EGRESS_PROXY_BINARY")]
    egress_proxy_binary: Option<PathBuf>,

    /// The fence helper executable that drives the network fence; this executable
    /// when unset.
    #[arg(long, env = "SANDBOX_FENCE_HELPER")]
    fence_helper: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    max_concurrent_leases: u32,

    #[command(flatten)]
    observability: trogon_sandbox_observability::ObservabilityArgs,
}

#[tokio::main]
pub async fn serve() -> anyhow::Result<()> {
    let args = Args::parse();
    let (_tracing, _registry) = trogon_sandbox_observability::bootstrap(
        "sandbox-subprocess-worker",
        trogon_sandbox_observability::SUBPROCESS_WORKER,
        &args.observability,
    )?;

    let nats = async_nats::connect(&args.nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {}", args.nats_url))?;
    let js = jetstream::new(nats.clone());

    let workspaces = trogon_sandbox_subprocess_worker::WorkspaceRegistry::new();
    let worker = WindowsWorker::new(WindowsWorkerConfig {
        agent_binary_default: args.agent_binary,
        agent_args: args.agent_args,
        workspace_root: args.workspace_root,
        egress_proxy_binary: args.egress_proxy_binary,
        fence_helper: args.fence_helper,
    })?
    .with_workspace_registry(workspaces.clone());
    let swept = worker.sweep_leftover_profiles();
    if swept > 0 {
        info!(swept, "deleted AppContainer profiles left by a crashed worker");
    }

    let cancel = CancellationToken::new();
    let worker_id = trogon_sandbox_contract::WorkerId::new(uuid::Uuid::now_v7().simple().to_string())
        .map_err(|error| anyhow::anyhow!("invalid worker id: {error}"))?;
    spawn_heartbeat_task(
        nats.clone(),
        worker_id.clone(),
        args.max_concurrent_leases,
        worker.active_lease_counter(),
        worker.profile().clone(),
        cancel.clone(),
    );
    spawn_shutdown_listener(cancel.clone())?;
    let health = args.observability.health();
    health.started_with_nats(&nats);
    health.shutdown_when(cancel.clone().cancelled_owned());

    let provider: Arc<dyn trogon_sandbox_protocol::Provider> = Arc::new(worker);
    info!("sandbox-subprocess-worker starting");
    // AppContainer leases have no namespace to enter, so nothing registers a
    // forward and every domain lookup answers that the lease holds none.
    let port_forwards = trogon_sandbox_subprocess_worker::port_forward::PortForwardRegistry::new();
    worker::run(nats, js, provider, workspaces, port_forwards, cancel, worker_id).await
}

fn spawn_shutdown_listener(cancel: CancellationToken) -> anyhow::Result<()> {
    let mut interrupt = ctrl_c()?;
    let mut break_key = ctrl_break()?;
    let mut close = ctrl_close()?;
    let mut shutdown = ctrl_shutdown()?;
    tokio::spawn(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = break_key.recv() => {}
            _ = close.recv() => {}
            _ = shutdown.recv() => {}
        }
        cancel.cancel();
    });
    Ok(())
}

#[cfg(test)]
mod tests;
