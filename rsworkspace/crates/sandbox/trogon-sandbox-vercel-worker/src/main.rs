use clap::Parser;
use trogon_sandbox_observability::{VERCEL_WORKER, bootstrap};
use trogon_sandbox_vercel_worker::{WorkerArgs, launch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let (_tracing, _registry) = bootstrap("trogon-sandbox-vercel-worker", VERCEL_WORKER, &args.observability)?;
    launch(&args, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
