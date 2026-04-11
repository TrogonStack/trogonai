use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info};
use trogon_xai_runner::XaiAgent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp".to_string());
    let default_model = std::env::var("XAI_DEFAULT_MODEL").unwrap_or_else(|_| "grok-3".to_string());
    let api_key = std::env::var("XAI_API_KEY").unwrap_or_else(|_| {
        info!("XAI_API_KEY not set; users must authenticate with their own key via 'xai-api-key'");
        String::new()
    });

    let system_prompt_set = std::env::var("XAI_SYSTEM_PROMPT").is_ok();
    let max_history_messages = std::env::var("XAI_MAX_HISTORY_MESSAGES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(20);
    let session_ttl_secs = std::env::var("XAI_SESSION_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(7 * 24 * 3600);
    let prompt_timeout_secs = std::env::var("XAI_PROMPT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(300);
    let models = std::env::var("XAI_MODELS")
        .unwrap_or_else(|_| "grok-3:Grok 3,grok-3-mini:Grok 3 Mini".to_string());
    let max_turns: Option<u32> = match std::env::var("XAI_MAX_TURNS") {
        Ok(s) => match s.parse::<u32>() {
            Ok(0) => None,
            Ok(n) => Some(n),
            Err(_) => Some(10),
        },
        Err(_) => Some(10),
    };
    info!(
        nats_url,
        prefix,
        default_model,
        models,
        system_prompt_set,
        max_history_messages,
        max_turns,
        prompt_timeout_secs,
        session_ttl_secs,
        "xai-runner starting"
    );

    let nats = async_nats::connect(&nats_url).await?;

    let acp_prefix = AcpPrefix::new(&prefix)?;
    let agent = XaiAgent::new(nats.clone(), acp_prefix.clone(), default_model, api_key).await?;

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) = AgentSideNatsConnection::new(agent, nats, acp_prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            info!("xai-runner listening on NATS");
            tokio::select! {
                result = io_task => result,
                _ = shutdown_signal() => {
                    info!("xai-runner received shutdown signal");
                    Ok(())
                }
            }
        })
        .await;

    match result {
        Ok(()) => info!("xai-runner stopped"),
        Err(ref e) => error!(error = %e, "xai-runner stopped with error"),
    }

    Ok(result?)
}

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
