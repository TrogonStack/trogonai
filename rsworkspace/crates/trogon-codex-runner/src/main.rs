use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info};
use trogon_codex_runner::CodexAgent;

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

    let acp_prefix = AcpPrefix::new(&prefix)?;
    let agent = CodexAgent::new(nats.clone(), &prefix, default_model);

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix, |fut| {
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
