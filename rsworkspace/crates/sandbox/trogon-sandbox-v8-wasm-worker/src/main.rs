use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_observability::{V8_WASM_WORKER, bootstrap};
use trogon_sandbox_protocol::register_fence_file;
use trogon_sandbox_v8_wasm_worker::{WorkerArgs, serve};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = WorkerArgs::parse();
    register_fence_file(args.dispatch_fence_file.as_deref());
    let (_tracing, _registry) = bootstrap("trogon-sandbox-v8-wasm-worker", V8_WASM_WORKER, &args.observability)?;
    let cancel = CancellationToken::new();
    let on_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_signal.cancel();
        }
    });
    Ok(serve(&args, cancel).await?)
}
