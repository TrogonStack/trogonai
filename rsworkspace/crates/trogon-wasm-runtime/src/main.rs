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

    // Fix 6: Validate configuration at startup.
    let validation_errors = cfg.validate();
    if !validation_errors.is_empty() {
        for err in &validation_errors {
            tracing::error!("Config error: {}", err);
        }
        return Err("Invalid configuration".into());
    }

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let acp_prefix =
        std::env::var("ACP_PREFIX").unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());

    info!(%nats_url, %acp_prefix, session_root = %cfg.session_root.display(), "WASM runtime starting");

    // Fix 1: Use ConnectOptions with indefinite reconnect.
    // Fix 2: Retry the initial connect in a loop — max_reconnects(None) only covers
    // reconnections after a successful first connect, not startup failures.
    let nats = {
        let mut attempts = 0u32;
        loop {
            match async_nats::ConnectOptions::new()
                .max_reconnects(None)
                .reconnect_delay_callback(|n| {
                    std::time::Duration::from_millis((100 * n as u64).min(5_000))
                })
                .connect(&nats_url)
                .await
            {
                Ok(c) => break c,
                Err(e) => {
                    attempts += 1;
                    let delay = std::time::Duration::from_millis((100 * attempts as u64).min(5_000));
                    tracing::warn!(error = %e, attempt = attempts, delay_ms = delay.as_millis(), "NATS connect failed, retrying");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    };
    let runtime = Rc::new(WasmRuntime::with_nats(&cfg, Some(nats.clone()))?);

    // Fix 2: Graceful shutdown via watch channel.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let shutdown_signal = acp_telemetry::signal::shutdown_signal();
            let mut dispatch_task =
                tokio::task::spawn_local(dispatcher::run(nats, acp_prefix, runtime, shutdown_rx));
            tokio::select! {
                result = &mut dispatch_task => {
                    match result {
                        Ok(()) => info!("Dispatcher exited"),
                        Err(e) => tracing::error!(error = %e, "Dispatcher task error"),
                    }
                }
                _ = shutdown_signal => {
                    info!("Shutdown signal received");
                    // Signal the dispatcher to stop accepting new messages and drain.
                    let _ = shutdown_tx.send(true);
                    // Wait for the dispatcher to finish draining.
                    let _ = dispatch_task.await;
                }
            }
        })
        .await;

    info!("WASM runtime stopped");
    Ok(())
}
