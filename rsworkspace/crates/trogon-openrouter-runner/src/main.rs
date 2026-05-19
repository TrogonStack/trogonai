mod config;
mod spawn_handler;

use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info, warn};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_openrouter_runner::{
    AgentLoader, NatsSessionNotifier, NatsSessionStore, OpenRouterAgent, SkillLoader,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let cfg = config::RunnerConfig::from_env();

    if cfg.api_key.is_none() {
        info!("OPENROUTER_API_KEY not set; users must authenticate with their own key via 'openrouter-api-key'");
    }

    info!(
        nats_url = cfg.nats_url,
        prefix = cfg.prefix,
        default_model = cfg.default_model,
        models = cfg.models_str,
        system_prompt_set = cfg.system_prompt_set,
        max_history_messages = cfg.max_history_messages,
        prompt_timeout_secs = cfg.prompt_timeout_secs,
        session_ttl_secs = cfg.session_ttl_secs,
        "openrouter-runner starting"
    );

    let nats = async_nats::connect(&cfg.nats_url).await?;
    let acp_prefix = AcpPrefix::new(&cfg.prefix)?;
    let js_ctx = async_nats::jetstream::new(nats.clone());
    let js = NatsJetStreamClient::new(js_ctx.clone());

    provision_streams(&js, &acp_prefix)
        .await
        .map_err(|e| format!("failed to provision JetStream streams: {e}"))?;

    // ── Registry self-registration ────────────────────────────────────────────

    let reg_store = trogon_registry::provision(&js_ctx)
        .await
        .map_err(|e| format!("registry provisioning failed: {e}"))?;
    let registry = trogon_registry::Registry::new(reg_store);
    let model_ids: Vec<String> = cfg
        .models_str
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect();
    let cap = trogon_registry::AgentCapability {
        agent_type: cfg.agent_type.clone(),
        capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
        nats_subject: format!("{}.agent.>", cfg.prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": &cfg.prefix, "models": model_ids }),
    };
    registry
        .register(&cap)
        .await
        .map_err(|e| format!("initial registry registration failed: {e}"))?;
    info!(agent_type = cfg.agent_type, prefix = cfg.prefix, "registered in agent registry");
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

    let spawn_key = cfg.api_key.clone().unwrap_or_default();
    let spawn_model = cfg.default_model.clone();
    let spawn_pfx = cfg.prefix.clone();

    let notifier = NatsSessionNotifier::new(nats.clone(), acp_prefix.clone());
    let mut agent = OpenRouterAgent::new(
        notifier,
        cfg.default_model,
        cfg.api_key.unwrap_or_default(),
    );

    {
        let js = js_ctx;

        if let Ok(agent_id) = std::env::var("AGENT_ID") {
            match (AgentLoader::open(&js).await, SkillLoader::open(&js).await) {
                (Ok(al), Ok(sl)) => {
                    info!(agent_id, "openrouter: console skill injection enabled");
                    agent = agent.with_loaders(agent_id, Arc::new(al), Arc::new(sl));
                }
                (Err(e), _) | (_, Err(e)) => {
                    warn!(error = %e, "openrouter: failed to open console KV buckets — skill injection disabled");
                }
            }
        }

        match NatsSessionStore::open(&js, cfg.session_ttl_secs).await {
            Ok(store) => {
                info!("openrouter: session persistence enabled");
                agent = agent.with_session_store(Arc::new(store));
            }
            Err(e) => {
                warn!(error = %e, "openrouter: failed to open SESSIONS KV bucket — session persistence disabled");
            }
        }
    }

    let spawn_nats = nats.clone();
    tokio::spawn(async move {
        use futures_util::StreamExt as _;
        let mut sub = spawn_nats
            .queue_subscribe(format!("{spawn_pfx}.agent.spawn"), "spawn-handlers".to_string())
            .await
            .expect("failed to subscribe to agent.spawn");
        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply else { continue };
            let Ok(req) = serde_json::from_slice::<serde_json::Value>(&msg.payload) else { continue };
            let prompt = req["prompt"].as_str().unwrap_or("").to_string();
            let nats2 = spawn_nats.clone();
            let key2 = spawn_key.clone();
            let model2 = spawn_model.clone();
            tokio::spawn(async move {
                let result = spawn_handler::oneshot_call(&key2, &model2, &prompt).await;
                nats2.publish(reply, result.into()).await.ok();
            });
        }
    });

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats, js, acp_prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            info!("openrouter-runner listening on NATS");
            tokio::select! {
                result = io_task => result,
                _ = shutdown_signal() => {
                    info!("openrouter-runner received shutdown signal");
                    Ok(())
                }
            }
        })
        .await;

    match result {
        Ok(()) => info!("openrouter-runner stopped"),
        Err(ref e) => error!(error = %e, "openrouter-runner stopped with error"),
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
