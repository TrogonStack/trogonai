//! `trogon-acp` — ACP server that routes prompts through NATS to `trogon-acp-runner`.
//!
//! ## Architecture
//!
//! ```text
//! ACP client (Zed / editor)
//!        ↓ stdio (newline-delimited JSON-RPC)
//!   trogon-acp  [this binary]
//!     Bridge<NatsClient>   ← acp-nats
//!        ↓↑ NATS Core (prompt publish / event subscribe)
//!   trogon-acp-runner      ← separate process or same process
//!     Runner               runs AgentLoop, publishes PromptEvents
//!        ↓
//!   Anthropic API (via trogon-secret-proxy)
//! ```
//!
//! ## Environment variables
//!
//! | Variable           | Default                  | Description                        |
//! |--------------------|--------------------------|-------------------------------------|
//! | `NATS_URL`         | `nats://localhost:4222`  | NATS server URL                    |
//! | `ACP_PREFIX`       | `acp`                    | NATS subject prefix for ACP        |
//! | `PROXY_URL`        | `http://localhost:8080`  | trogon-secret-proxy base URL       |
//! | `ANTHROPIC_TOKEN`  | —                        | Proxy token for Anthropic API      |
//! | `GITHUB_TOKEN`     | —                        | Proxy token for GitHub API         |
//! | `LINEAR_TOKEN`     | —                        | Proxy token for Linear API         |
//! | `SLACK_TOKEN`      | —                        | Proxy token for Slack API          |
//! | `AGENT_MODEL`      | `claude-opus-4-6`        | Claude model ID                    |
//! | `AGENT_MAX_ITERATIONS` | `10`                 | Max loop iterations per prompt     |

use std::sync::Arc;

use acp_nats::{AcpPrefix, Bridge, Config};
use agent_client_protocol::{AgentSideConnection, Client, SessionNotification};
use async_nats::jetstream;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::info;

use trogon_agent::agent_loop::AgentLoop;
use trogon_agent::tools::ToolContext;
use trogon_acp_runner::Runner;
use trogon_nats::NatsConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "trogon_acp=info,acp_nats=info,trogon_acp_runner=info".into()),
        )
        .with_writer(std::io::stderr) // keep stdout clean for ACP protocol
        .init();

    // ── Config from environment ───────────────────────────────────────────────

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let acp_prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp".to_string());
    let proxy_url = std::env::var("PROXY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let anthropic_token = std::env::var("ANTHROPIC_TOKEN").unwrap_or_default();
    let github_token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    let linear_token = std::env::var("LINEAR_TOKEN").unwrap_or_default();
    let slack_token = std::env::var("SLACK_TOKEN").unwrap_or_default();
    let model = std::env::var("AGENT_MODEL").unwrap_or_else(|_| "claude-opus-4-6".to_string());
    let max_iterations: u32 = std::env::var("AGENT_MAX_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    // ── NATS connection ───────────────────────────────────────────────────────

    let nats = async_nats::connect(&nats_url).await?;
    info!(url = %nats_url, "connected to NATS");

    let js = jetstream::new(nats.clone());

    // ── AgentLoop ─────────────────────────────────────────────────────────────

    let http_client = reqwest::Client::new();
    let tool_context = Arc::new(ToolContext {
        http_client: http_client.clone(),
        proxy_url: proxy_url.clone(),
        github_token: github_token.clone(),
        linear_token: linear_token.clone(),
        slack_token: slack_token.clone(),
    });

    let agent_loop = AgentLoop {
        http_client,
        proxy_url,
        anthropic_token,
        model,
        max_iterations,
        tool_context,
        memory_owner: None,
        memory_repo: None,
        memory_path: None,
        mcp_tool_defs: vec![],
        mcp_dispatch: vec![],
        split_client: None,
        tenant_id: "default".to_string(),
    };

    // ── Runner (NATS subscriber + agent) ─────────────────────────────────────

    let runner = Runner::new(nats.clone(), &js, agent_loop, acp_prefix.clone()).await?;
    tokio::spawn(async move { runner.run().await });

    // ── Bridge (ACP ↔ NATS) ──────────────────────────────────────────────────

    let (notification_tx, mut notification_rx) = mpsc::channel::<SessionNotification>(64);

    let nats_config = NatsConfig {
        servers: vec![nats_url],
        auth: trogon_nats::NatsAuth::None,
    };
    let config = Config::new(AcpPrefix::new(acp_prefix)?, nats_config);

    let meter = opentelemetry::global::meter("trogon-acp");
    let bridge = Bridge::new(
        nats.clone(),
        trogon_std::time::SystemClock,
        &meter,
        config,
        notification_tx,
    );

    // ── ACP connection over stdio ─────────────────────────────────────────────

    let local = LocalSet::new();

    local
        .run_until(async move {
            let stdin = tokio::io::stdin().compat();
            let stdout = tokio::io::stdout().compat_write();

            let (conn, io_task) =
                AgentSideConnection::new(bridge, stdout, stdin, |fut| {
                    tokio::task::spawn_local(fut);
                });

            // Forward session notifications from the bridge to the ACP client
            tokio::task::spawn_local(async move {
                while let Some(notification) = notification_rx.recv().await {
                    if let Err(e) = conn.session_notification(notification).await {
                        tracing::warn!(error = %e, "failed to forward session notification");
                    }
                }
            });

            if let Err(e) = io_task.await {
                tracing::warn!(error = %e, "ACP IO task ended");
            }
        })
        .await;

    Ok(())
}
