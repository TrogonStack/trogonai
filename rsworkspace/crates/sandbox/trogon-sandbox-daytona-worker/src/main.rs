use clap::Parser;
use trogon_sandbox_daytona_worker::{WorkerArgs, launch};
use trogon_sandbox_observability::{DAYTONA_WORKER, bootstrap};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let (_tracing, _registry) = bootstrap("trogon-sandbox-daytona-worker", DAYTONA_WORKER, &args.observability)?;
    launch(&args, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
