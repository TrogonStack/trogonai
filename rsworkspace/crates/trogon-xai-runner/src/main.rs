use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info, warn};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_xai_runner::{
    AgentLoader, NatsSessionNotifier, NatsSessionStore, SkillLoader, XaiAgent,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp".to_string());
    let default_model = std::env::var("XAI_DEFAULT_MODEL").unwrap_or_else(|_| "grok-4".to_string());
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
        .unwrap_or_else(|_| "grok-4:Grok 4,grok-3:Grok 3,grok-3-mini:Grok 3 Mini".to_string());
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
    let js_ctx = async_nats::jetstream::new(nats.clone());
    let js = NatsJetStreamClient::new(js_ctx.clone());

    provision_streams(&js, &acp_prefix)
        .await
        .map_err(|e| format!("failed to provision JetStream streams: {e}"))?;

    // ── Registry self-registration ────────────────────────────────────────────

    let agent_type = std::env::var("AGENT_TYPE").unwrap_or_else(|_| "xai".to_string());
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
    let registry_for_agent = registry.clone();
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

    let notifier = NatsSessionNotifier::new(nats.clone(), acp_prefix.clone());
    let mut agent = XaiAgent::new(notifier, default_model, api_key)
        .with_execution_backend(nats.clone(), registry_for_agent);

    // If AGENT_ID is set, attach console skill loaders so skills defined in
    // trogon-console are injected into every new session's system prompt.
    // Also open the SESSIONS KV bucket for session visibility in trogon-console.
    {
        let js = js_ctx;

        if let Ok(agent_id) = std::env::var("AGENT_ID") {
            match (AgentLoader::open(&js).await, SkillLoader::open(&js).await) {
                (Ok(al), Ok(sl)) => {
                    info!(agent_id, "xai: console skill injection enabled");
                    agent = agent.with_loaders(agent_id, Arc::new(al), Arc::new(sl));
                }
                (Err(e), _) | (_, Err(e)) => {
                    warn!(error = %e, "xai: failed to open console KV buckets — skill injection disabled");
                }
            }
        }

        match NatsSessionStore::open(&js, session_ttl_secs).await {
            Ok(store) => {
                info!("xai: session persistence enabled");
                agent = agent.with_session_store(Arc::new(store));
            }
            Err(e) => {
                warn!(error = %e, "xai: failed to open SESSIONS KV bucket — session persistence disabled");
            }
        }
    }

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats, js, acp_prefix, |fut| {
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
