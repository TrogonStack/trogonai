//! NATS subscriber loop — wires incoming events to agent handlers.
//!
//! The runner subscribes to two NATS subjects and dispatches each message to
//! the appropriate handler:
//!
//! | Subject               | Handler                          |
//! |-----------------------|----------------------------------|
//! | `github.pull_request` | [`handlers::pr_review::handle`]  |
//! | `linear.Issue`        | [`handlers::issue_triage::handle`]|
//!
//! The runner never panics on individual message errors — it logs and
//! continues.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tracing::{error, info};

use crate::agent_loop::AgentLoop;
use crate::config::AgentConfig;
use crate::handlers::{self, make_tool_context};
use crate::tools::ToolContext;

#[derive(Debug)]
pub enum RunnerError {
    Nats(trogon_nats::ConnectError),
    NatsSubscribe(async_nats::SubscribeError),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nats(e) => write!(f, "NATS connect error: {e}"),
            Self::NatsSubscribe(e) => write!(f, "NATS subscribe error: {e}"),
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<trogon_nats::ConnectError> for RunnerError {
    fn from(e: trogon_nats::ConnectError) -> Self {
        Self::Nats(e)
    }
}

impl From<async_nats::SubscribeError> for RunnerError {
    fn from(e: async_nats::SubscribeError) -> Self {
        Self::NatsSubscribe(e)
    }
}

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Connect to NATS and start processing events.
pub async fn run(cfg: AgentConfig) -> Result<(), RunnerError> {
    let nats = trogon_nats::connect(&cfg.nats, NATS_CONNECT_TIMEOUT).await?;

    let http_client = reqwest::Client::new();

    let tool_ctx: Arc<ToolContext> = make_tool_context(
        http_client.clone(),
        cfg.proxy_url.clone(),
        cfg.github_token.clone(),
        cfg.linear_token.clone(),
    );

    let agent = Arc::new(AgentLoop {
        http_client,
        proxy_url: cfg.proxy_url.clone(),
        anthropic_token: cfg.anthropic_token.clone(),
        model: cfg.model.clone(),
        max_iterations: cfg.max_iterations,
        tool_context: tool_ctx,
    });

    let mut pr_sub = nats.subscribe("github.pull_request").await?;
    let mut issue_sub = nats.subscribe("linear.Issue").await?;

    info!(
        proxy_url = %cfg.proxy_url,
        model = %cfg.model,
        "Agent runner started"
    );

    loop {
        tokio::select! {
            msg = pr_sub.next() => {
                let Some(msg) = msg else { break };
                let agent = Arc::clone(&agent);
                tokio::spawn(async move {
                    match handlers::pr_review::handle(&agent, &msg.payload).await {
                        Some(Ok(output)) => info!(output = %output, "PR review done"),
                        Some(Err(e))     => error!(error = %e, "PR review error"),
                        None             => {}
                    }
                });
            }
            msg = issue_sub.next() => {
                let Some(msg) = msg else { break };
                let agent = Arc::clone(&agent);
                tokio::spawn(async move {
                    match handlers::issue_triage::handle(&agent, &msg.payload).await {
                        Some(Ok(output)) => info!(output = %output, "Issue triage done"),
                        Some(Err(e))     => error!(error = %e, "Issue triage error"),
                        None             => {}
                    }
                });
            }
        }
    }

    info!("Agent runner stopped (subscriptions closed)");
    Ok(())
}
