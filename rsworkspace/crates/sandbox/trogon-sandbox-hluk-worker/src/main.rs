use std::error::Error;
use std::sync::Arc;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_hluk_worker::{HlukCli, HlukProvider, WorkerArgs, build_heartbeat, serve};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args = WorkerArgs::parse();
    let client = async_nats::connect(&args.nats_url).await?;
    tokio::fs::create_dir_all(&args.work_dir).await?;
    let provider = Arc::new(HlukProvider::new(HlukCli::new(args.hluk_bin), args.work_dir));
    let worker_id = WorkerId::new(uuid::Uuid::new_v4().to_string())?;
    let cancel = CancellationToken::new();
    let (heartbeat_id, heartbeat_provider) = (worker_id.clone(), Arc::clone(&provider));
    let max = args.max_concurrent_leases;
    spawn_heartbeat_loop(
        client.clone(),
        ProviderKind::Hluk,
        worker_id.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let current = u32::try_from(heartbeat_provider.lease_count()).unwrap_or(u32::MAX);
            let heartbeat = build_heartbeat(&heartbeat_id, max, current);
            async move { heartbeat }
        },
    );
    let on_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_signal.cancel();
        }
    });
    serve(client, provider, worker_id, cancel).await?;
    Ok(())
}
