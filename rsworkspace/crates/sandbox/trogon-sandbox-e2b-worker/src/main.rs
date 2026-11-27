use clap::Parser;
use trogon_sandbox_e2b_worker::{WorkerArgs, launch};
use trogon_sandbox_observability::{E2B_WORKER, bootstrap};
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let (_tracing, _registry) = bootstrap("trogon-sandbox-e2b-worker", E2B_WORKER, &args.observability)?;
    launch(&args, &SystemEnv, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
