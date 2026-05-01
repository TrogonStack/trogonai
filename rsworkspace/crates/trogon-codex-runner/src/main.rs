use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info, warn};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_codex_runner::DefaultCodexAgent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp".to_string());
    let default_model =
        std::env::var("CODEX_DEFAULT_MODEL").unwrap_or_else(|_| "o4-mini".to_string());

    info!(nats_url, prefix, default_model, "codex-runner starting");

    let nats = async_nats::connect(&nats_url).await?;
    let js_ctx = async_nats::jetstream::new(nats.clone());

    // ── JetStream stream provisioning ─────────────────────────────────────────

    let acp_prefix_parsed = AcpPrefix::new(&prefix)?;
    provision_streams(&NatsJetStreamClient::new(js_ctx.clone()), &acp_prefix_parsed)
        .await
        .map_err(|e| format!("failed to provision JetStream streams: {e}"))?;

    // ── Registry self-registration ────────────────────────────────────────────

    let agent_type = std::env::var("AGENT_TYPE").unwrap_or_else(|_| "codex".to_string());
    let reg_store = trogon_registry::provision(&js_ctx).await
        .map_err(|e| format!("registry provisioning failed: {e}"))?;
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.clone(),
        capabilities: vec!["chat".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": &prefix }),
    };
    registry.register(&cap).await
        .map_err(|e| format!("initial registry registration failed: {e}"))?;
    info!(agent_type, prefix, "registered in agent registry");
    tokio::spawn({
        let cap = cap.clone();
        async move {
            let mut interval = tokio::time::interval(trogon_registry::HEARTBEAT_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = registry.refresh(&cap).await {
                    warn!(error = %e, "registry heartbeat failed");
                }
            }
        }
    });

    let agent = DefaultCodexAgent::with_nats(nats.clone(), acp_prefix_parsed.clone(), default_model);

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix_parsed, |fut| {
                tokio::task::spawn_local(fut);
            });
            info!("codex-runner listening on NATS");
            tokio::select! {
                result = io_task => result,
                _ = shutdown_signal() => {
                    info!("codex-runner received shutdown signal");
                    Ok(())
                }
            }
        })
        .await;

    match result {
        Ok(()) => info!("codex-runner stopped"),
        Err(ref e) => error!(error = %e, "codex-runner stopped with error"),
    }

    Ok(result?)
}

/// Resolves on SIGINT (Ctrl+C) or SIGTERM (container shutdown).
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
