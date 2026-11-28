#[path = "main/constants.rs"]
mod constants;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use constants::DEFAULT_AGENT_PORT;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_actor_agent::{
    ACTOR_ID_PATH, ActorIdentitySource, AgentState, MIN_SUPPORTED_WORKER_PROTOCOL_VERSION,
    MIN_WORKER_PROTOCOL_VERSION_ENV, router, serve, serve_vsock, try_min_protocol_version_from_env,
};
use trogon_std::env::SystemEnv;

#[derive(Debug, Parser)]
#[command(
    name = "trogon-sandbox-actor-agent",
    about = "In-actor HTTP agent for the sandbox provider"
)]
struct Args {
    #[arg(long, env = "SANDBOX_ACTOR_AGENT_PORT", default_value_t = DEFAULT_AGENT_PORT)]
    port: u16,

    #[arg(long, env = "SANDBOX_ACTOR_AGENT_BIND", default_value = "0.0.0.0")]
    bind: IpAddr,

    #[arg(long, env = "SANDBOX_ACTOR_AGENT_ACTOR_ID_PATH", default_value = ACTOR_ID_PATH)]
    actor_id_path: PathBuf,

    /// Listen on AF_VSOCK (any CID) instead of TCP.
    #[arg(long, env = "SANDBOX_ACTOR_AGENT_VSOCK_PORT")]
    vsock_port: Option<u32>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let minimum = try_min_protocol_version_from_env(
        &SystemEnv,
        MIN_WORKER_PROTOCOL_VERSION_ENV,
        MIN_SUPPORTED_WORKER_PROTOCOL_VERSION,
    )?;
    let state = Arc::new(
        AgentState::new(ActorIdentitySource::new(args.actor_id_path)).with_min_worker_protocol_version(minimum),
    );
    let cancel = CancellationToken::new();
    let shutdown = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            shutdown.cancel();
        }
    });

    let app = router(state);
    match args.vsock_port {
        Some(port) => serve_vsock(port, app, cancel).await?,
        None => {
            let addr = SocketAddr::new(args.bind, args.port);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            info!(%addr, "actor agent listening");
            serve(listener, app, cancel).await?;
        }
    }
    Ok(())
}
