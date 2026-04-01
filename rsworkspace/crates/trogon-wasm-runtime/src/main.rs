mod dispatcher;

use std::rc::Rc;
use tracing::info;
use trogon_wasm_runtime::{Config, WasmRuntime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("trogon_wasm_runtime=info".parse().unwrap()),
        )
        .init();

    let cfg = Config::from_env();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let acp_prefix =
        std::env::var("ACP_PREFIX").unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());

    info!(%nats_url, %acp_prefix, session_root = %cfg.session_root.display(), "WASM runtime starting");

    let nats = async_nats::connect(&nats_url).await?;
    let runtime = Rc::new(WasmRuntime::new(&cfg)?);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let shutdown = acp_telemetry::signal::shutdown_signal();
            let mut dispatch_task =
                tokio::task::spawn_local(dispatcher::run(nats, acp_prefix, runtime));
            tokio::select! {
                result = &mut dispatch_task => {
                    match result {
                        Ok(()) => info!("Dispatcher exited"),
                        Err(e) => tracing::error!(error = %e, "Dispatcher task error"),
                    }
                }
                _ = shutdown => {
                    info!("Shutdown signal received");
                    dispatch_task.abort();
                    let _ = dispatch_task.await;
                }
            }
        })
        .await;

    info!("WASM runtime stopped");
    Ok(())
}
