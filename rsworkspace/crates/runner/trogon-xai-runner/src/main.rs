use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use tracing::{error, info, warn};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_xai_runner::{AgentLoader, NatsSessionNotifier, SkillLoader, XaiAgent, open_default_session_store};

#[tokio::main]
#[allow(clippy::expect_used)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    // MED-34: default to a runner-specific prefix so the spawn subscriber does not
    // share `acp.agent.spawn` with the openrouter runner (which would round-robin
    // spawn requests to the wrong backend). The dev script still overrides this.
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp.grok".to_string());
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
    let reg_store = trogon_registry::provision(&js_ctx)
        .await
        .map_err(|e| format!("registry provisioning failed: {e}"))?;
    let registry = trogon_registry::Registry::new(reg_store);
    let model_ids: Vec<String> = models
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect();
    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.clone(),
        capabilities: trogon_registry::RunnerCapability::to_strings(
            trogon_registry::expected_runner_capabilities("xai").expect("xai capabilities"),
        ),
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": &prefix, "models": model_ids }),
    };
    let registry_for_agent = registry.clone();
    let registry_for_register = registry.clone();
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

    let _spawn_api_key = api_key.clone();
    let _spawn_model = default_model.clone();
    let _spawn_prefix = prefix.clone();

    let nats_config = acp_nats::NatsConfig {
        servers: vec![nats_url.clone()],
        auth: acp_nats::NatsAuth::None,
    };
    let runner_config = acp_nats::Config::new(acp_prefix.clone(), nats_config);

    let notifier = NatsSessionNotifier::new(nats.clone(), acp_prefix.clone());
    let proxy_url = std::env::var("PROXY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let anthropic_token = std::env::var("ANTHROPIC_TOKEN").unwrap_or_default();
    let anthropic_base_url = std::env::var("ANTHROPIC_BASE_URL").ok();
    let classifier_http = reqwest::Client::new();
    let _safety_classifier = trogon_runner_tools::build_auto_safety_classifier(
        classifier_http,
        &proxy_url,
        anthropic_base_url.as_deref(),
        anthropic_token,
    );
    let mut agent = XaiAgent::new(notifier, default_model, api_key)
        .with_execution_backend(nats.clone(), registry_for_agent)
        .with_compactor(nats.clone())
        .with_permissions(nats.clone(), acp_prefix.clone());
    if let Ok(catalog) =
        trogonai_catalog_client::open(&js_ctx, trogonai_catalog_client::CatalogClientConfig::default()).await
    {
        agent = agent.with_catalog(catalog);
    }
    let mut agent = agent.with_runner_config(runner_config);

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

        agent = agent.with_default_session_store(open_default_session_store(&js, session_ttl_secs).await);
    }

    let local = tokio::task::LocalSet::new();
    let (perm_tx, mut perm_rx) = tokio::sync::mpsc::channel::<trogon_runner_tools::PermissionReq>(32);
    let perm_store = trogon_runner_tools::AllowedToolsSessionStore::new();
    agent = agent.with_permission_gate(perm_tx, perm_store.clone());
    let nats_for_perm = nats.clone();
    let prefix_for_perm = acp_prefix.clone();

    let (elic_tx, mut elic_rx) = tokio::sync::mpsc::channel::<trogon_runner_tools::ElicitationReq>(32);
    agent = agent.with_elicitation(elic_tx);
    let nats_for_elic = nats.clone();
    let prefix_for_elic = acp_prefix.clone();

    let result = local
        .run_until(async {
            tokio::task::spawn_local(async move {
                while let Some(req) = perm_rx.recv().await {
                    trogon_runner_tools::handle_permission_request_nats(
                        req,
                        nats_for_perm.clone(),
                        prefix_for_perm.clone(),
                        &perm_store,
                    )
                    .await;
                }
            });

            tokio::task::spawn_local(async move {
                while let Some(req) = elic_rx.recv().await {
                    trogon_runner_tools::handle_elicitation_request_nats(
                        req,
                        nats_for_elic.clone(),
                        prefix_for_elic.clone(),
                    )
                    .await;
                }
            });

            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(agent, nats, js, acp_prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            // LOW-16: register AFTER the ACP subscription is established so
            // incoming requests are not received before we can handle them.
            if let Err(e) = registry_for_register.register(&cap).await {
                return Err(acp_nats_agent::ConnectionError::Subscribe(
                    format!("initial registry registration failed: {e}").into(),
                ));
            }
            info!(agent_type, prefix, "registered in agent registry");
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
#[allow(clippy::expect_used)]
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
