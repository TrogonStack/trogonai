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
use trogon_runner_tools::session_store::SessionStore as _;

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
    let tool_context = Arc::new(ToolContext {
        proxy_url: proxy_url.clone(),
        cwd,
        http_client: http_client.clone(),
    });

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
    .with_execution_backend(nats.clone(), registry_for_agent);

    let prefix = AcpPrefix::new(&acp_prefix)?;
    let nats_for_perm = nats.clone();
    let nats_for_elic = nats.clone();
    let nats_for_spawn = nats.clone();
    let js_for_spawn = js.clone();
    let prefix_for_perm = prefix.clone();
    let prefix_for_elic = prefix.clone();
    let acp_prefix_for_spawn = acp_prefix.clone();
    let runner_config = acp_nats::Config::new(
        prefix.clone(),
        trogon_nats::NatsConfig {
            servers: vec![nats_url.clone()],
            auth: trogon_nats::NatsAuth::None,
        },
    );
    let js_client = NatsJetStreamClient::new(js);
    let (_conn, io_task) =
        AgentSideNatsConnection::with_jetstream(agent, nats, js_client, acp_prefix_parsed, |fut| {
            tokio::task::spawn_local(fut);
        });

    info!("trogon-acp-runner started");

    let local = LocalSet::new();
    local
        .run_until(async move {
            let store_for_spawn = store.clone();

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

            // Handle spawn_agent requests: create a sub-session, run a prompt, return output.
            tokio::task::spawn_local({
                let nats = nats_for_spawn;
                let js = js_for_spawn;
                let config = runner_config;
                let store = store_for_spawn;
                let acp_prefix_str = acp_prefix_for_spawn;
                async move {
                    use agent_client_protocol::SessionUpdate;
                    use futures_util::StreamExt;

                    let subject = format!("{acp_prefix_str}.agent.spawn");
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

                        if session_id.is_empty() {
                            nats.publish(reply, "spawn_agent: missing session_id".into()).await.ok();
                            continue;
                        }

                        let parent_state = match store.load(&session_id).await {
                            Ok(s) => s,
                            Err(_) => {
                                nats.publish(reply, "spawn_agent: session not found".into()).await.ok();
                                continue;
                            }
                        };

                        const MAX_SPAWN_DEPTH: u32 = 3;
                        if parent_state.spawn_depth >= MAX_SPAWN_DEPTH {
                            nats.publish(reply, "spawn_agent: max nesting depth reached".into()).await.ok();
                            continue;
                        }

                        let worktree = trogon_runner_tools::worktree::create_worktree(&parent_state.cwd).await;
                        let sub_cwd = worktree
                            .as_ref()
                            .map(|w| w.path.clone())
                            .unwrap_or_else(|| parent_state.cwd.clone());

                        // Dummy channel: Bridge's notification_sender will silently fail (warn and continue).
                        let (notif_tx, _notif_rx_bridge_unused) = tokio::sync::mpsc::channel::<agent_client_protocol::SessionNotification>(1);
                        drop(_notif_rx_bridge_unused);

                        // Clone nats before moving into Bridge so we can subscribe below.
                        let nats_for_notif = nats.clone();
                        let acp_prefix_for_notif = acp_prefix_str.clone();

                        let bridge = acp_nats::Bridge::new(
                            nats.clone(),
                            NatsJetStreamClient::new(js.clone()),
                            trogon_std::time::SystemClock,
                            &opentelemetry::global::meter("trogon-acp-runner"),
                            config.clone(),
                            notif_tx,
                        );

                        let sub_sid = match trogon_runner_tools::spawn_session::create_sub_session(
                            &bridge,
                            &sub_cwd,
                            &parent_state.mode,
                            parent_state.permission_rules_text.as_deref(),
                        ).await {
                            Ok(sid) => sid,
                            Err(e) => {
                                drop(worktree);
                                nats.publish(reply, format!("spawn_agent error: {e}").into()).await.ok();
                                continue;
                            }
                        };

                        if let Ok(mut sub_state) = store.load(&sub_sid).await {
                            sub_state.spawn_depth = parent_state.spawn_depth + 1;
                            let _ = store.save(&sub_sid, &sub_state).await;
                        }

                        // Subscribe directly to the plain-NATS subject the runner publishes to.
                        let notif_subject = format!("{acp_prefix_for_notif}.session.{sub_sid}.client.session.update");
                        let mut notif_sub = match nats_for_notif.subscribe(notif_subject).await {
                            Ok(s) => s,
                            Err(e) => {
                                drop(worktree);
                                nats.publish(reply, format!("spawn_agent error: subscribe failed: {e}").into()).await.ok();
                                continue;
                            }
                        };

                        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
                        let text_result = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
                        let text_clone = text_result.clone();

                        tokio::task::spawn_local(async move {
                            loop {
                                tokio::select! {
                                    biased;
                                    msg = notif_sub.next() => {
                                        let Some(msg) = msg else { break; };
                                        if let Ok(notif) = serde_json::from_slice::<agent_client_protocol::SessionNotification>(&msg.payload)
                                            && let SessionUpdate::AgentMessageChunk(ref chunk) = notif.update
                                            && let agent_client_protocol::ContentBlock::Text(ref t) = chunk.content
                                        {
                                            text_clone.lock().await.push_str(&t.text);
                                        }
                                    }
                                    _ = &mut stop_rx => {
                                        // Drain remaining buffered messages with a short timeout.
                                        while let Ok(Some(msg)) = tokio::time::timeout(
                                            std::time::Duration::from_millis(50),
                                            notif_sub.next(),
                                        ).await {
                                            if let Ok(notif) = serde_json::from_slice::<agent_client_protocol::SessionNotification>(&msg.payload)
                                                && let SessionUpdate::AgentMessageChunk(ref chunk) = notif.update
                                                && let agent_client_protocol::ContentBlock::Text(ref t) = chunk.content
                                            {
                                                text_clone.lock().await.push_str(&t.text);
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                        });

                        let run_result = trogon_runner_tools::spawn_session::run_sub_session(
                            &bridge,
                            &sub_sid,
                            &task,
                            std::time::Duration::from_secs(3600),
                        ).await;

                        drop(worktree);

                        let _ = stop_tx.send(());
                        // Give the notification task a moment to finish draining.
                        tokio::task::yield_now().await;

                        let accumulated = text_result.lock().await.clone();
                        let text = match run_result {
                            Ok(()) => if accumulated.is_empty() {
                                "Sub-agent completed.".to_string()
                            } else {
                                accumulated
                            },
                            Err(e) => format!("spawn_agent error: {e}"),
                        };

                        nats.publish(reply, text.into()).await.ok();
                    }
                }
            });

            io_task.await
        })
        .await?;

    Ok(())
}
