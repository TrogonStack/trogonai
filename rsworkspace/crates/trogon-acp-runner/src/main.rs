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
use acp_nats_agent::AgentSideNatsConnection;
use async_nats::jetstream;
use tokio::sync::{mpsc, RwLock};
use tokio::task::LocalSet;
use tracing::info;

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

    // ── NATS connection ───────────────────────────────────────────────────────

    let nats = async_nats::connect(&nats_url).await?;
    info!(url = %nats_url, "connected to NATS");

    let js = jetstream::new(nats.clone());

    // ── AgentLoop ─────────────────────────────────────────────────────────────

    let http_client = reqwest::Client::new();
    let tool_context = Arc::new(ToolContext {
        http_client: http_client.clone(),
        proxy_url: proxy_url.clone(),
    });

    let mut agent_loop = AgentLoop {
        http_client,
        proxy_url,
        anthropic_token,
        anthropic_base_url: None,
        anthropic_extra_headers: vec![],
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
    .with_compactor(nats.clone());

    let prefix = AcpPrefix::new(&acp_prefix)?;
    let nats_for_perm = nats.clone();
    let nats_for_elic = nats.clone();
    let prefix_for_perm = prefix.clone();
    let prefix_for_elic = prefix.clone();
    let (_conn, io_task) =
        AgentSideNatsConnection::new(agent, nats, prefix, |fut| {
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

            io_task.await
        })
        .await?;

    Ok(())
}
