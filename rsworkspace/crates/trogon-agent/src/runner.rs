//! JetStream pull-consumer runner — wires incoming events to agent handlers.
//!
//! The runner binds two durable pull consumers and dispatches each message to
//! the appropriate handler.  Messages are acked only after the handler returns,
//! giving at-least-once delivery if the agent crashes mid-processing.
//!
//! | JetStream stream | Filter subject       | Handler                            |
//! |------------------|----------------------|------------------------------------|
//! | `GITHUB`         | `github.pull_request`| [`handlers::pr_review::handle`]    |
//! | `LINEAR`         | `linear.Issue.>`     | [`handlers::issue_triage::handle`] |
//!
//! Stream names and filter subjects match the defaults used by `trogon-github`
//! and `trogon-linear`.  They can be overridden via env vars
//! `GITHUB_STREAM_NAME` and `LINEAR_STREAM_NAME`.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, consumer::pull, consumer::AckPolicy, consumer::DeliverPolicy};
use futures_util::StreamExt;
use tracing::{error, info, warn};

use crate::agent_loop::AgentLoop;
use crate::config::{AgentConfig, McpServerConfig};
use crate::handlers::{self, make_tool_context};
use crate::tools::{ToolContext, ToolDef};

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Durable consumer name for the GitHub pull_request stream.
const GITHUB_CONSUMER: &str = "trogon-agent-pr-review";
/// Durable consumer name for the GitHub issue_comment stream.
const COMMENT_CONSUMER: &str = "trogon-agent-comment-added";
/// Durable consumer name for the GitHub push stream.
const PUSH_CONSUMER: &str = "trogon-agent-push-to-branch";
/// Durable consumer name for the GitHub check_run stream.
const CI_CONSUMER: &str = "trogon-agent-ci-completed";
/// Durable consumer name for the Linear Issue stream.
const LINEAR_CONSUMER: &str = "trogon-agent-issue-triage";

#[derive(Debug)]
pub enum RunnerError {
    Nats(trogon_nats::ConnectError),
    JetStream(String),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nats(e) => write!(f, "NATS connect error: {e}"),
            Self::JetStream(e) => write!(f, "JetStream error: {e}"),
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<trogon_nats::ConnectError> for RunnerError {
    fn from(e: trogon_nats::ConnectError) -> Self {
        Self::Nats(e)
    }
}

/// Connect to NATS and start processing events from JetStream.
pub async fn run(cfg: AgentConfig) -> Result<(), RunnerError> {
    let nats = trogon_nats::connect(&cfg.nats, NATS_CONNECT_TIMEOUT).await?;
    let js = jetstream::new(nats);

    let http_client = reqwest::Client::new();

    let tool_ctx: Arc<ToolContext> = make_tool_context(
        http_client.clone(),
        cfg.proxy_url.clone(),
        cfg.github_token.clone(),
        cfg.linear_token.clone(),
    );

    let (mcp_tool_defs, mcp_dispatch) =
        init_mcp_servers(&http_client, &cfg.mcp_servers).await;

    let agent = Arc::new(AgentLoop {
        http_client,
        proxy_url: cfg.proxy_url.clone(),
        anthropic_token: cfg.anthropic_token.clone(),
        model: cfg.model.clone(),
        max_iterations: cfg.max_iterations,
        tool_context: tool_ctx,
        memory_owner: cfg.memory_owner.clone(),
        memory_repo: cfg.memory_repo.clone(),
        memory_path: cfg.memory_path.clone(),
        mcp_tool_defs,
        mcp_dispatch,
    });

    let github_stream_name = cfg.github_stream_name.as_deref().unwrap_or("GITHUB");
    let linear_stream_name = cfg.linear_stream_name.as_deref().unwrap_or("LINEAR");

    let mut pr_messages = bind_consumer(
        &js,
        github_stream_name,
        GITHUB_CONSUMER,
        "github.pull_request",
    )
    .await?;

    let mut comment_messages = bind_consumer(
        &js,
        github_stream_name,
        COMMENT_CONSUMER,
        "github.issue_comment",
    )
    .await?;

    let mut push_messages = bind_consumer(
        &js,
        github_stream_name,
        PUSH_CONSUMER,
        "github.push",
    )
    .await?;

    let mut ci_messages = bind_consumer(
        &js,
        github_stream_name,
        CI_CONSUMER,
        "github.check_run",
    )
    .await?;

    let mut issue_messages = bind_consumer(
        &js,
        linear_stream_name,
        LINEAR_CONSUMER,
        "linear.Issue.>",
    )
    .await?;

    info!(
        proxy_url = %cfg.proxy_url,
        model = %cfg.model,
        github_stream = github_stream_name,
        linear_stream = linear_stream_name,
        "Agent runner started"
    );

    loop {
        tokio::select! {
            msg = pr_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving PR message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        tokio::spawn(async move {
                            // Route merged vs open/sync based on action + merged flag.
                            let is_merged = serde_json::from_slice::<serde_json::Value>(&msg.payload)
                                .map(|v| v["action"].as_str() == Some("closed")
                                    && v["pull_request"]["merged"].as_bool() == Some(true))
                                .unwrap_or(false);

                            if is_merged {
                                match handlers::pr_merged::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "PR merged done"),
                                    Some(Err(e)) => error!(error = %e, "PR merged error"),
                                    None => {}
                                }
                            } else {
                                match handlers::pr_review::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "PR review done"),
                                    Some(Err(e)) => error!(error = %e, "PR review error"),
                                    None => {}
                                }
                            }
                            if let Err(e) = msg.ack().await {
                                warn!(error = %e, "Failed to ack PR message");
                            }
                        });
                    }
                }
            }
            msg = comment_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving comment message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        tokio::spawn(async move {
                            match handlers::comment_added::handle(&agent, &msg.payload).await {
                                Some(Ok(o)) => info!(output = %o, "Comment-added done"),
                                Some(Err(e)) => error!(error = %e, "Comment-added error"),
                                None => {}
                            }
                            if let Err(e) = msg.ack().await {
                                warn!(error = %e, "Failed to ack comment message");
                            }
                        });
                    }
                }
            }
            msg = push_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving push message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        tokio::spawn(async move {
                            match handlers::push_to_branch::handle(&agent, &msg.payload).await {
                                Some(Ok(o)) => info!(output = %o, "Push-to-branch done"),
                                Some(Err(e)) => error!(error = %e, "Push-to-branch error"),
                                None => {}
                            }
                            if let Err(e) = msg.ack().await {
                                warn!(error = %e, "Failed to ack push message");
                            }
                        });
                    }
                }
            }
            msg = ci_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving CI message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        tokio::spawn(async move {
                            match handlers::ci_completed::handle(&agent, &msg.payload).await {
                                Some(Ok(o)) => info!(output = %o, "CI-completed done"),
                                Some(Err(e)) => error!(error = %e, "CI-completed error"),
                                None => {}
                            }
                            if let Err(e) = msg.ack().await {
                                warn!(error = %e, "Failed to ack CI message");
                            }
                        });
                    }
                }
            }
            msg = issue_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving issue message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        tokio::spawn(async move {
                            match handlers::issue_triage::handle(&agent, &msg.payload).await {
                                Some(Ok(o)) => info!(output = %o, "Issue triage done"),
                                Some(Err(e)) => error!(error = %e, "Issue triage error"),
                                None => {}
                            }
                            if let Err(e) = msg.ack().await {
                                warn!(error = %e, "Failed to ack issue message");
                            }
                        });
                    }
                }
            }
        }
    }

    info!("Agent runner stopped");
    Ok(())
}

/// Connect to each configured MCP server, discover tools, and build the
/// tool definitions + dispatch table for the [`AgentLoop`].
///
/// Servers that fail to initialize are skipped with a warning so a single
/// misconfigured server cannot block the agent from starting.
async fn init_mcp_servers(
    http_client: &reqwest::Client,
    servers: &[McpServerConfig],
) -> (Vec<ToolDef>, Vec<(String, String, Arc<trogon_mcp::McpClient>)>) {
    let mut tool_defs = Vec::new();
    let mut dispatch = Vec::new();

    for server in servers {
        let client = Arc::new(trogon_mcp::McpClient::new(
            http_client.clone(),
            &server.url,
        ));

        if let Err(e) = client.initialize().await {
            warn!(server = %server.name, error = %e, "Failed to initialize MCP server — skipping");
            continue;
        }

        match client.list_tools().await {
            Err(e) => warn!(server = %server.name, error = %e, "Failed to list MCP tools — skipping server"),
            Ok(tools) => {
                info!(server = %server.name, count = tools.len(), "MCP tools loaded");
                for tool in tools {
                    let prefixed = format!(
                        "mcp__{}__{}", sanitize_name(&server.name), sanitize_name(&tool.name)
                    );
                    tool_defs.push(ToolDef {
                        name: prefixed.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                        cache_control: None,
                    });
                    dispatch.push((prefixed, tool.name, Arc::clone(&client)));
                }
            }
        }
    }

    (tool_defs, dispatch)
}

/// Replace characters not valid in Anthropic tool names with `_`.
fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize_name;

    #[test]
    fn sanitize_name_keeps_alphanumeric_and_dash() {
        assert_eq!(sanitize_name("my-server"), "my-server");
        assert_eq!(sanitize_name("server123"), "server123");
        assert_eq!(sanitize_name("abc-XYZ-789"), "abc-XYZ-789");
    }

    #[test]
    fn sanitize_name_replaces_spaces_and_special_chars() {
        assert_eq!(sanitize_name("my server"), "my_server");
        assert_eq!(sanitize_name("my.server"), "my_server");
        assert_eq!(sanitize_name("my/tool"), "my_tool");
        assert_eq!(sanitize_name("tool:v2"), "tool_v2");
    }

    #[test]
    fn sanitize_name_empty_string() {
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn sanitize_name_all_special_chars() {
        assert_eq!(sanitize_name("..."), "___");
    }
}

async fn bind_consumer(
    js: &jetstream::Context,
    stream_name: &str,
    consumer_name: &str,
    filter_subject: &str,
) -> Result<impl futures_util::Stream<Item = Result<jetstream::Message, async_nats::error::Error<jetstream::consumer::pull::MessagesErrorKind>>>, RunnerError> {
    let stream = js
        .get_stream(stream_name)
        .await
        .map_err(|e| RunnerError::JetStream(format!("get stream {stream_name}: {e}")))?;

    let consumer: jetstream::consumer::Consumer<pull::Config> = stream
        .get_or_create_consumer(
            consumer_name,
            pull::Config {
                durable_name: Some(consumer_name.to_string()),
                filter_subject: filter_subject.to_string(),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::All,
                max_deliver: 3,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| RunnerError::JetStream(format!("create consumer {consumer_name}: {e}")))?;

    consumer
        .messages()
        .await
        .map_err(|e| RunnerError::JetStream(format!("messages stream {consumer_name}: {e}")))
}
