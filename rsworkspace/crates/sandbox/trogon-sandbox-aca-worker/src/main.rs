use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_aca_worker::{WorkerArgs, serve};
use trogon_sandbox_observability::{ACA_WORKER, bootstrap};
use trogon_sandbox_protocol::register_fence_file;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = WorkerArgs::parse();
    register_fence_file(args.dispatch_fence_file.as_deref());
    let (_tracing, _registry) = bootstrap("trogon-sandbox-aca-worker", ACA_WORKER, &args.observability)?;
    let cancel = CancellationToken::new();
    let on_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_signal.cancel();
        }
    });
    Ok(serve(&args, cancel).await?)
}
