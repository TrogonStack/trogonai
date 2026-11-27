use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_observability::{SNAPSHOT_BUILDER, bootstrap};
use trogon_sandbox_snapshot_builder::{SnapshotBuilderArgs, maybe_run_extractor, serve};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The builder re-runs itself to unpack untrusted images under resource limits.
    if let Some(result) = maybe_run_extractor() {
        return Ok(result?);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = SnapshotBuilderArgs::parse();
    let (_tracing, registry) = bootstrap("trogon-sandbox-snapshot-builder", SNAPSHOT_BUILDER, &args.observability)?;
    let cancel = CancellationToken::new();
    let on_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_signal.cancel();
        }
    });
    Ok(serve(&args, &registry, cancel).await?)
}
