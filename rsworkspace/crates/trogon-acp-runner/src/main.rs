//! `trogon-acp-runner` — standalone ACP runner server.
//!
//! ## Architecture
//!
//! ```text
//! ACP client (Zed / editor)
//!        ↓ WebSocket
//!   acp-nats-ws  (dumb pipe: ACP ↔ NATS)
//!        ↓↑ NATS request-reply / pub-sub
//!   trogon-acp-runner  [this binary]
//!     ├─ RpcServer   — initialize / authenticate / new_session / load_session
//!     │               set_session_mode / set_session_config_option / list_sessions
//!     │               fork_session / resume_session
//!     └─ Runner      — prompt / cancel (streaming via PromptEvent pub-sub)
//!          ↓
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

use async_nats::jetstream;
use tokio::sync::RwLock;
use tracing::info;

use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "trogon_acp_runner=info,acp_nats=info".into()
            }),
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
        model,
        max_iterations,
        tool_context,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        permission_checker: None,
        thinking_budget,
    };

    if let Some(budget) = thinking_budget {
        agent_loop.thinking_budget = Some(budget);
    }

    // ── Shared gateway config ─────────────────────────────────────────────────

    let gateway_config = Arc::new(RwLock::new(None::<trogon_acp_runner::GatewayConfig>));

    // ── Session store ─────────────────────────────────────────────────────────

    let store = trogon_acp_runner::SessionStore::open(&js).await?;

    // ── RpcServer (handles all non-prompt ACP methods) ────────────────────────

    let rpc_server = trogon_acp_runner::RpcServer::new(
        nats.clone(),
        store.clone(),
        acp_prefix.clone(),
        gateway_config.clone(),
    );
    tokio::spawn(async move { rpc_server.run().await });

    // ── Runner (handles prompt / cancel) ─────────────────────────────────────

    let runner = trogon_acp_runner::Runner::new(
        nats,
        &js,
        agent_loop,
        acp_prefix,
        None, // no in-process permission gate — auto-allow all tools
        gateway_config,
    )
    .await?;

    info!("trogon-acp-runner started");
    runner.run().await;

    Ok(())
}
