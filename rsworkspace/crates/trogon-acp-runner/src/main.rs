//! `trogon-acp-runner` — standalone ACP runner server.
#![cfg_attr(coverage, feature(coverage_attribute))]
//!
//! ## Architecture
//!
//! ```text
//! ACP client (Zed / editor)
//!        ↓ WebSocket
//!   acp-nats-ws  (dumb pipe: ACP ↔ NATS)
//!        ↓↑ NATS request-reply / pub-sub
//!   trogon-acp-runner  [this binary]
//!     └─ TrogonAgent (implements Agent trait)
//!          ├─ initialize / authenticate / new_session / load_session
//!          │  set_session_mode / set_session_config_option / list_sessions
//!          │  fork_session / resume_session / close_session
//!          └─ prompt / cancel (streaming via NatsClientProxy)
//!               ↓
//!     Anthropic API (via trogon-secret-proxy)
//! ```
//!
//! ## Environment variables
//!
//! | Variable               | Default                 | Description                      |
//! |------------------------|-------------------------|----------------------------------|
//! | `NATS_URL`             | `nats://localhost:4222` | NATS server URL                  |
//! | `ACP_PREFIX`           | `acp`                   | NATS subject prefix for ACP      |
//! | `PROXY_URL`            | `http://localhost:8080` | trogon-secret-proxy base URL     |
//! | `ANTHROPIC_TOKEN`      | —                       | Proxy token for Anthropic API    |
//! | `AGENT_MODEL`          | `claude-opus-4-6`       | Claude model ID                  |
//! | `AGENT_MAX_ITERATIONS` | `10`                    | Max loop iterations per prompt   |
//! | `MAX_THINKING_TOKENS`  | —                       | Extended thinking token budget   |

use std::sync::Arc;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::jetstream::provision::provision_streams;
use acp_nats_agent::AgentSideNatsConnection;
use async_nats::jetstream;
use tokio::sync::{mpsc, RwLock};
use tokio::task::LocalSet;
use tracing::info;
use trogon_nats::jetstream::NatsJetStreamClient;

use trogon_acp_runner::elicitation::handle_elicitation_request_nats;
use trogon_acp_runner::permission_bridge::handle_permission_request_nats;
use trogon_acp_runner::{ElicitationReq, PermissionReq};

use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;

#[cfg_attr(coverage, coverage(off))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trogon_acp_runner=info,acp_nats=info".into()),
        )
        .init();

    // ── Config from environment ───────────────────────────────────────────────

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let acp_prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp".to_string());
    let proxy_url =
        std::env::var("PROXY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let anthropic_token = std::env::var("ANTHROPIC_TOKEN").unwrap_or_default();
    let model = std::env::var("AGENT_MODEL").unwrap_or_else(|_| "claude-opus-4-6".to_string());
    let max_iterations: u32 = std::env::var("AGENT_MAX_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let thinking_budget: Option<u32> = std::env::var("MAX_THINKING_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok());
    let anthropic_base_url: Option<String> = std::env::var("ANTHROPIC_BASE_URL").ok();

    // ── NATS connection ───────────────────────────────────────────────────────

    let nats = async_nats::connect(&nats_url).await?;
    info!(url = %nats_url, "connected to NATS");

    let js = jetstream::new(nats.clone());

    // ── JetStream stream provisioning ─────────────────────────────────────────

    let acp_prefix_parsed = AcpPrefix::new(&acp_prefix)?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &acp_prefix_parsed)
        .await
        .map_err(|e| anyhow::anyhow!("failed to provision JetStream streams: {e}"))?;

    // ── Registry self-registration ────────────────────────────────────────────

    let agent_type = std::env::var("AGENT_TYPE").unwrap_or_else(|_| "claude".to_string());
    let reg_store = trogon_registry::provision(&js).await
        .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
    let registry = trogon_registry::Registry::new(reg_store);
    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.clone(),
        capabilities: vec!["chat".to_string(), "code_edit".to_string()],
        nats_subject: format!("{}.agent.>", acp_prefix),
        current_load: 0,
        metadata: serde_json::json!({
            "acp_prefix": &acp_prefix,
            "models": ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5-20251001"],
        }),
    };
    registry.register(&cap).await
        .map_err(|e| anyhow::anyhow!("initial registry registration failed: {e}"))?;
    info!(agent_type, acp_prefix, "registered in agent registry");
    let registry_for_agent = registry.clone();
    tokio::spawn({
        let cap = cap.clone();
        async move {
            let mut interval = tokio::time::interval(trogon_registry::HEARTBEAT_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = registry.refresh(&cap).await {
                    tracing::warn!(error = %e, "registry heartbeat failed");
                }
            }
        }
    });

    // ── AgentLoop ─────────────────────────────────────────────────────────────

    let http_client = reqwest::Client::new();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let (web_search_api_key, web_search_endpoint) = trogon_tools::web_search_config_from_env();
    let tool_context = Arc::new(ToolContext {
        proxy_url: proxy_url.clone(),
        cwd,
        http_client: http_client.clone(),
        web_search_api_key,
        web_search_endpoint,
    });

    // `auto`-mode LLM safety classifier — uses the same proxy creds as the agent
    // loop, with a small/cheap model. Built before the creds move into AgentLoop.
    let safety_classifier = trogon_runner_tools::build_auto_safety_classifier(
        http_client.clone(),
        &proxy_url,
        anthropic_base_url.as_deref(),
        anthropic_token.clone(),
    );

    let mut agent_loop = AgentLoop {
        http_client,
        proxy_url,
        anthropic_token,
        anthropic_base_url,
        anthropic_extra_headers: vec![],
        streaming_client: None,
        model: model.clone(),
        max_iterations,
        tool_context,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        elicitation_provider: None,
        post_tool_observer: None,
        thinking_budget,
    };

    if let Some(budget) = thinking_budget {
        agent_loop.thinking_budget = Some(budget);
    }

    // ── Shared gateway config ─────────────────────────────────────────────────

    let gateway_config = Arc::new(RwLock::new(None::<trogon_acp_runner::GatewayConfig>));

    // ── Permission gate channel ───────────────────────────────────────────────
    // TrogonAgent sends PermissionReq over this channel; the LocalSet task below
    // handles each request by calling NatsClientProxy::request_permission.

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(32);

    // ── Elicitation channel ───────────────────────────────────────────────────
    // When the agent needs structured user input, it sends ElicitationReq here;
    // the LocalSet task forwards it to the ACP client via NatsClientProxy.
    // The sender is kept alive (currently inert) for future phases that wire
    // an agent-side ElicitationProvider.

    let (elic_tx, mut elic_rx) = mpsc::channel::<ElicitationReq>(32);

    // ── Session store ─────────────────────────────────────────────────────────

    let store = trogon_acp_runner::NatsSessionStore::open(&js).await?;

    // ── NATS notifier ─────────────────────────────────────────────────────────

    let notifier = trogon_acp_runner::NatsSessionNotifier::new(nats.clone());

    // ── TrogonAgent ───────────────────────────────────────────────────────────

    let agent = trogon_acp_runner::TrogonAgent::new(
        notifier,
        store.clone(),
        agent_loop,
        acp_prefix.clone(),
        model,
        Some(perm_tx),
        Some(elic_tx),
        gateway_config,
    )
    .with_compactor(nats.clone())
    .with_execution_backend(nats.clone(), registry_for_agent)
    .with_safety_classifier(safety_classifier);

    // Cheap shared handle for the spawn_agent responder. It runs sub-agents
    // in-process (see TrogonAgent::spawn_subagent) rather than round-tripping a
    // prompt back through NATS into this same runner, which deadlocks.
    let agent_for_spawn = agent.clone();

    let prefix = AcpPrefix::new(&acp_prefix)?;
    let nats_for_perm = nats.clone();
    let nats_for_elic = nats.clone();
    let nats_for_spawn = nats.clone();
    let prefix_for_perm = prefix.clone();
    let prefix_for_elic = prefix.clone();
    let acp_prefix_for_spawn = acp_prefix.clone();

    let js_client = NatsJetStreamClient::new(js);
    let (_conn, io_task) =
        AgentSideNatsConnection::with_jetstream(agent, nats, js_client, acp_prefix_parsed, |fut| {
            tokio::task::spawn_local(fut);
        });

    info!("trogon-acp-runner started");

    let local = LocalSet::new();
    local
        .run_until(async move {
            // Drain PermissionReq messages and forward each to the ACP client via NATS.
            tokio::task::spawn_local(async move {
                while let Some(req) = perm_rx.recv().await {
                    let nats = nats_for_perm.clone();
                    let prefix = prefix_for_perm.clone();
                    handle_permission_request_nats(req, nats, prefix, &store).await;
                }
            });

            // Drain ElicitationReq messages and forward each to the ACP client via NATS.
            tokio::task::spawn_local(async move {
                while let Some(req) = elic_rx.recv().await {
                    let nats = nats_for_elic.clone();
                    let prefix = prefix_for_elic.clone();
                    handle_elicitation_request_nats(req, nats, prefix).await;
                }
            });

            // Handle spawn_agent requests by running the sub-agent IN-PROCESS:
            // TrogonAgent::spawn_subagent creates an isolated worktree, opens an
            // ephemeral session inheriting the parent's context, drives the full
            // tool-use loop directly (no NATS round-trip into this same runner —
            // that deadlocks), and returns the sub-agent's text. Each request is
            // handled on its own task so the responder keeps draining the queue.
            tokio::task::spawn_local({
                let nats = nats_for_spawn;
                let acp_prefix_str = acp_prefix_for_spawn;
                let agent = agent_for_spawn;
                async move {
                    use futures_util::StreamExt;

                    // `{prefix}.spawn`, NOT `{prefix}.agent.spawn`: the latter falls under
                    // the global ACP wildcard `{prefix}.agent.>` and would be double-handled
                    // (and raced with an error reply) by serve_global. Must match the subject
                    // SpawnAgentTool sends to.
                    let subject = format!("{acp_prefix_str}.spawn");
                    let mut sub = match nats.subscribe(subject).await {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(error = %e, "spawn handler: failed to subscribe");
                            return;
                        }
                    };

                    while let Some(msg) = sub.next().await {
                        let Some(reply) = msg.reply.clone() else { continue };
                        let Ok(req) = serde_json::from_slice::<serde_json::Value>(&msg.payload) else {
                            nats.publish(reply, "spawn_agent: invalid JSON payload".into()).await.ok();
                            continue;
                        };

                        let task = req["prompt"].as_str().unwrap_or("").to_string();
                        let session_id = req["session_id"].as_str().unwrap_or("").to_string();
                        let agent_name = req["agent"].as_str().unwrap_or("").to_string();

                        if session_id.is_empty() {
                            nats.publish(reply, "spawn_agent: missing session_id".into()).await.ok();
                            continue;
                        }

                        let agent = agent.clone();
                        let nats = nats.clone();
                        tokio::task::spawn_local(async move {
                            let text = match agent.spawn_subagent(&session_id, &task, &agent_name).await {
                                Ok(text) => text,
                                Err(err) => err,
                            };
                            nats.publish(reply, text.into()).await.ok();
                        });
                    }
                }
            });

            io_task.await
        })
        .await?;

    Ok(())
}
