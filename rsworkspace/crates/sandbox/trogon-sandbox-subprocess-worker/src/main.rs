#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn main() {
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        b"trogon-sandbox-subprocess-worker requires Linux, macOS or Windows\n",
    );
    std::process::exit(1);
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod worker;

#[cfg(windows)]
mod windows_serve;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args();
    let argv0 = args.next().unwrap_or_default();
    if args.next().as_deref() == Some("--fence-helper") {
        let rest = std::iter::once(argv0).chain(args);
        std::process::exit(trogon_sandbox_fence_helper::run_from_args(rest));
    }
    windows_serve::serve()
}

#[cfg(target_os = "linux")]
use std::io::Write as _;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context as _;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use async_nats::jetstream;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use clap::Parser;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use tokio_util::sync::CancellationToken;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use tracing::{info, warn};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_sandbox_contract::WorkerId;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_sandbox_egress_proxy::{NatsViolationSink, ViolationRateLimit};
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::isolation::{guest_init, landlock, nested, observer, relay};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_sandbox_subprocess_worker::lease_secrets::NatsCredentialSources;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_sandbox_subprocess_worker::{IsolationBackendKind, SubprocessWorker, WorkerConfig};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use trogon_std::env::SystemEnv;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox subprocess worker: serves sandbox lease requests through isolated local subprocesses"
)]
struct Args {
    /// Journal for the tenant and attempt each lease belongs to, so the
    /// dispatch fence survives a restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    dispatch_fence_file: Option<std::path::PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(long, env = "SANDBOX_AGENT_BINARY")]
    agent_binary: String,

    #[arg(long, env = "SANDBOX_WORKSPACE_ROOT")]
    workspace_root: PathBuf,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    max_concurrent_leases: usize,

    #[cfg_attr(
        target_os = "linux",
        arg(long, env = "SANDBOX_ISOLATION_BACKEND", default_value = "bubblewrap")
    )]
    #[cfg_attr(
        target_os = "macos",
        arg(long, env = "SANDBOX_ISOLATION_BACKEND", default_value = "seatbelt")
    )]
    isolation_backend: IsolationBackendKind,

    #[cfg(target_os = "linux")]
    #[arg(long, env = "SANDBOX_BWRAP_BINARY")]
    bwrap_binary_path: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_EGRESS_PROXY_BINARY")]
    egress_proxy_binary_path: Option<PathBuf>,

    /// Refuse a lease that needs credentials when the control plane has not
    /// committed a credential policy for it. Off, such a lease runs with
    /// unfenced credential requests.
    #[arg(long, env = "SANDBOX_REQUIRE_COMMITTED_POLICY", default_value_t = false)]
    require_committed_policy: bool,

    #[command(flatten)]
    observability: trogon_sandbox_observability::ObservabilityArgs,
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    serve()
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    let argv: Vec<_> = std::env::args_os().collect();
    if guest_init::is_init_invocation(&argv) {
        match guest_init::run(guest_init::InitArgs::parse()) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                fail("init", &err, guest_init::INIT_EXIT_CODE);
            }
        }
    }
    if nested::is_nested_invocation(&argv) {
        match nested::run(nested::NestedArgs::parse()) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                fail("nested", &err, nested::NESTED_EXIT_CODE);
            }
        }
    }
    if landlock::is_confine_invocation(&argv) {
        match landlock::run(landlock::ConfineArgs::parse()) {
            Ok(never) => match never {},
            Err(err) => {
                fail("confine", &err, landlock::CONFINE_EXIT_CODE);
            }
        }
    }
    if observer::is_observe_invocation(&argv) {
        std::process::exit(observer::run(observer::ObserveArgs::parse())?);
    }
    if trogon_sandbox_subprocess_worker::namespace_bridge::is_bridge_invocation(&argv) {
        std::process::exit(trogon_sandbox_subprocess_worker::namespace_bridge::run_bridge(&argv));
    }
    if relay::is_relay_invocation(&argv) {
        std::process::exit(relay::run(relay::RelayArgs::parse())?);
    }
    serve()
}

/// Reports why a re-executed helper mode failed and ends the process with its exit code.
#[cfg(target_os = "linux")]
fn fail(mode: &str, error: &anyhow::Error, code: i32) -> ! {
    let _ = writeln!(std::io::stderr(), "{mode}: {error:#}");
    std::process::exit(code)
}

#[cfg(target_os = "linux")]
fn bwrap_binary_path(args: &Args) -> Option<PathBuf> {
    args.bwrap_binary_path.clone()
}

#[cfg(target_os = "macos")]
fn bwrap_binary_path(_args: &Args) -> Option<PathBuf> {
    None
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::main]
async fn serve() -> anyhow::Result<()> {
    let args = Args::parse();
    trogon_sandbox_protocol::register_fence_file(args.dispatch_fence_file.as_deref());
    let (_tracing, _registry) = trogon_sandbox_observability::bootstrap(
        "trogon-sandbox-subprocess-worker",
        trogon_sandbox_observability::SUBPROCESS_WORKER,
        &args.observability,
    )?;

    let nats = async_nats::connect(&args.nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {}", args.nats_url))?;
    let js = jetstream::new(nats.clone());

    let bwrap_binary_path = bwrap_binary_path(&args);
    let config = WorkerConfig {
        agent_binary_default: PathBuf::from(args.agent_binary),
        workspace_root: args.workspace_root,
        isolation_backend: args.isolation_backend,
        bwrap_binary_path,
        egress_proxy_binary_path: args.egress_proxy_binary_path,
        relay_binary_path: None,
        fuse: trogon_sandbox_subprocess_worker::FuseConfig::from_env(&SystemEnv)?,
    };
    let workspaces = trogon_sandbox_subprocess_worker::WorkspaceRegistry::new();
    let terminals =
        trogon_sandbox_subprocess_worker::terminal::SubprocessTerminals::new(Arc::new(nats.clone()), &SystemEnv);
    let port_forwards = trogon_sandbox_subprocess_worker::port_forward::PortForwardRegistry::new();
    let worker = SubprocessWorker::new(config)?
        .with_workspace_registry(workspaces.clone())
        .with_port_forwards(port_forwards.clone())
        .with_terminals(Arc::clone(&terminals))
        .with_credentials(Arc::new(NatsCredentialSources::new(args.nats_url.clone())))
        .with_committed_fence(trogon_sandbox_credential_broker::CommittedFence::new(
            Arc::new(trogon_sandbox_credential_broker::NatsKvCommittedPolicies::new(
                trogon_sandbox_credential_broker::NatsKvCommittedPolicies::open_bucket(&js).await?,
            )),
            args.require_committed_policy,
        ))
        .with_fs_violation_sink(
            Arc::new(NatsViolationSink::new(nats.clone())),
            ViolationRateLimit::default(),
        );
    let current_leases = worker.active_lease_counter();
    let worker = Arc::new(worker);
    let provider: Arc<dyn trogon_sandbox_protocol::Provider> = Arc::clone(&worker) as _;

    let cancel = CancellationToken::new();
    let authority_worker = WorkerId::new(uuid::Uuid::now_v7().simple().to_string())
        .map_err(|error| anyhow::anyhow!("invalid worker id: {error}"))?;
    let authority_guard = trogon_sandbox_protocol::AuthorityGuard::new(
        trogon_sandbox_protocol::AuthorityConfig::new(
            trogon_sandbox_protocol::AuthorityMode::from_env(&SystemEnv).map_err(|error| anyhow::anyhow!(error))?,
        ),
        authority_worker.clone(),
        Arc::new(trogon_sandbox_protocol::NatsAuthorityOracle::new(
            nats.clone(),
            authority_worker.clone(),
        )),
        Arc::new(trogon_sandbox_subprocess_worker::worker_authority::SubprocessAuthorityHost::new(Arc::clone(&worker))),
    );
    tokio::spawn(authority_guard.run(cancel.clone()));
    // Without a heartbeat the broker never learns this worker's profile and
    // cannot place a lease on it.
    trogon_sandbox_subprocess_worker::heartbeat::spawn_heartbeat_task(
        nats.clone(),
        authority_worker.clone(),
        u32::try_from(args.max_concurrent_leases).unwrap_or(u32::MAX),
        current_leases,
        trogon_sandbox_subprocess_worker::profile_for_backend(args.isolation_backend),
        cancel.clone(),
    );
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_clone.cancel();
        }
    });
    let health = args.observability.health();
    health.started_with_nats(&nats);
    health.shutdown_when(cancel.clone().cancelled_owned());

    let ssh_bridge = trogon_sandbox_ssh_gateway::SshStreamBridge::new(
        Arc::new(trogon_sandbox_subprocess_worker::ssh_bridge::SubprocessSshHost(
            Arc::clone(&terminals),
        )),
        nats.clone(),
        trogon_sandbox_ssh_gateway::BridgeLimits::default(),
    );
    let ssh_serve = tokio::spawn(ssh_bridge.serve(nats.clone(), cancel.clone()));
    let terminal_serve = tokio::spawn(terminals.serve(nats.clone(), cancel.clone()));

    info!("trogon-sandbox-subprocess-worker starting");
    let served = worker::run(
        nats,
        js,
        provider,
        workspaces,
        port_forwards,
        cancel.clone(),
        authority_worker,
    )
    .await;
    cancel.cancel();
    if let Ok(Err(e)) = ssh_serve.await {
        warn!(error = %e, "ssh stream service stopped");
    }
    if let Ok(Err(e)) = terminal_serve.await {
        warn!(error = %e, "terminal service stopped");
    }
    served
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "main_tests.rs"]
mod tests;
