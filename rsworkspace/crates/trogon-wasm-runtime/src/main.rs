use std::rc::Rc;
use tracing::{info, warn};
use trogon_wasm_runtime::{dispatcher, Config, WasmRuntime};
use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use trogon_nats::jetstream::NatsJetStreamClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("trogon_wasm_runtime=info".parse().unwrap()),
        )
        .init();

    let cfg = Config::from_env();

    let validation_errors = cfg.validate();
    if !validation_errors.is_empty() {
        for err in &validation_errors {
            tracing::error!("Config error: {}", err);
        }
        return Err("Invalid configuration".into());
    }

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let acp_prefix =
        std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp.wasm".to_string());
    let agent_type =
        std::env::var("AGENT_TYPE").unwrap_or_else(|_| "wasm".to_string());

    info!(%nats_url, %acp_prefix, %agent_type, session_root = %cfg.session_root.display(), "WASM runtime starting");

    // Use ConnectOptions with indefinite reconnect.
    // Retry the initial connect in a loop — max_reconnects(None) only covers
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
                    let delay =
                        std::time::Duration::from_millis((100 * attempts as u64).min(5_000));
                    tracing::warn!(error = %e, attempt = attempts, delay_ms = delay.as_millis(), "NATS connect failed, retrying");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    };

    let runtime = Rc::new(WasmRuntime::with_nats(&cfg, Some(nats.clone()))?);
    runtime.cleanup_stale_sessions().await;

    // ── JetStream streams + registry ─────────────────────────────────────────

    let js_ctx = async_nats::jetstream::new(nats.clone());
    let acp_prefix_typed = AcpPrefix::new(&acp_prefix)
        .map_err(|e| format!("invalid ACP_PREFIX '{acp_prefix}': {e}"))?;
    let js = NatsJetStreamClient::new(js_ctx.clone());
    provision_streams(&js, &acp_prefix_typed)
        .await
        .map_err(|e| format!("failed to provision JetStream streams: {e}"))?;

    let reg_store = trogon_registry::provision(&js_ctx)
        .await
        .map_err(|e| format!("registry provisioning failed: {e}"))?;
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.clone(),
        capabilities: vec!["execution".to_string()],
        nats_subject: format!("{acp_prefix}.agent.>"),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": &acp_prefix }),
    };
    registry
        .register(&cap)
        .await
        .map_err(|e| format!("initial registry registration failed: {e}"))?;
    info!(agent_type, acp_prefix, "registered in agent registry");
    tokio::spawn({
        let cap = cap.clone();
        async move {
            let mut interval =
                tokio::time::interval(trogon_registry::HEARTBEAT_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = registry.refresh(&cap).await {
                    warn!(error = %e, "registry heartbeat failed");
                }
            }
        }
    });

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let shutdown_signal = trogon_std::signal::shutdown_signal();
            // Subscribe to client-op subjects for this runtime's ACP prefix
            // (e.g. acp.wasm.session.*.client.*) so tool calls from wasm sessions
            // are handled by this instance.
            let subject = format!("{acp_prefix}.session.*.client.>");
            let mut dispatch_task =
                tokio::task::spawn_local(dispatcher::run(nats, subject, runtime, shutdown_rx));
            tokio::select! {
                result = &mut dispatch_task => {
                    match result {
                        Ok(()) => info!("Dispatcher exited"),
                        Err(e) => tracing::error!(error = %e, "Dispatcher task error"),
                    }
                }
                _ = shutdown_signal => {
                    info!("Shutdown signal received");
                    let _ = shutdown_tx.send(true);
                    let _ = dispatch_task.await;
                }
            }
        })
        .await;

    info!("WASM runtime stopped");
    Ok(())
}
