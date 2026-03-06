//! JetStream pull-consumer runner — wires incoming events to agent handlers.
//!
//! ## Dispatch strategy
//!
//! For each incoming NATS event the runner:
//!
//! 1. Queries the [`AutomationStore`] for enabled automations whose `trigger`
//!    matches the event's subject and payload action.
//! 2. If one or more automations match, **each is executed in a separate
//!    `tokio::spawn`** (parallel execution).
//! 3. If **no** automations match, the runner falls back to the built-in
//!    hardcoded handler so the agent works out-of-the-box without any
//!    configuration.
//!
//! This means automations can be added, edited, enabled, or disabled at any
//! time via the [`trogon_automations::api`] HTTP API — no restart required.
//!
//! | JetStream stream | Filter subject        | Fallback handler                   |
//! |------------------|-----------------------|------------------------------------|
//! | `GITHUB`         | `github.pull_request` | pr_review / pr_merged              |
//! | `GITHUB`         | `github.issue_comment`| comment_added                      |
//! | `GITHUB`         | `github.push`         | push_to_branch                     |
//! | `GITHUB`         | `github.check_run`    | ci_completed                       |
//! | `LINEAR`         | `linear.Issue.>`      | issue_triage                       |

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, consumer::pull, consumer::AckPolicy, consumer::DeliverPolicy};
use futures_util::StreamExt;
use tracing::{error, info, warn};
use trogon_automations::AutomationStore;

use crate::agent_loop::AgentLoop;
use crate::config::{AgentConfig, McpServerConfig};
use crate::handlers::{self, make_tool_context};
use crate::tools::{ToolContext, ToolDef};

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const GITHUB_CONSUMER: &str = "trogon-agent-pr-review";
const COMMENT_CONSUMER: &str = "trogon-agent-comment-added";
const PUSH_CONSUMER: &str = "trogon-agent-push-to-branch";
const CI_CONSUMER: &str = "trogon-agent-ci-completed";
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

pub async fn run(cfg: AgentConfig) -> Result<(), RunnerError> {
    let nats = trogon_nats::connect(&cfg.nats, NATS_CONNECT_TIMEOUT).await?;
    let js = jetstream::new(nats);

    let http_client = reqwest::Client::new();

    let tool_ctx: Arc<ToolContext> = make_tool_context(
        http_client.clone(),
        cfg.proxy_url.clone(),
        cfg.github_token.clone(),
        cfg.linear_token.clone(),
        cfg.slack_token.clone(),
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

    let store = Arc::new(
        AutomationStore::open(&js)
            .await
            .map_err(|e| RunnerError::JetStream(format!("AutomationStore: {e}")))?,
    );

    // Start the automations HTTP API server unless disabled (port == 0).
    if cfg.api_port != 0 {
        let api_state = trogon_automations::api::AppState { store: (*store).clone() };
        let api_port = cfg.api_port;
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(("0.0.0.0", api_port)).await {
                Err(e) => tracing::error!(port = api_port, error = %e, "Failed to bind automations API"),
                Ok(listener) => {
                    tracing::info!(port = api_port, "Automations API listening");
                    if let Err(e) = axum::serve(listener, trogon_automations::api::router(api_state)).await {
                        tracing::error!(error = %e, "Automations API server error");
                    }
                }
            }
        });
    }

    let github_stream_name = cfg.github_stream_name.as_deref().unwrap_or("GITHUB");
    let linear_stream_name = cfg.linear_stream_name.as_deref().unwrap_or("LINEAR");

    let mut pr_messages = bind_consumer(&js, github_stream_name, GITHUB_CONSUMER, "github.pull_request").await?;
    let mut comment_messages = bind_consumer(&js, github_stream_name, COMMENT_CONSUMER, "github.issue_comment").await?;
    let mut push_messages = bind_consumer(&js, github_stream_name, PUSH_CONSUMER, "github.push").await?;
    let mut ci_messages = bind_consumer(&js, github_stream_name, CI_CONSUMER, "github.check_run").await?;
    let mut issue_messages = bind_consumer(&js, linear_stream_name, LINEAR_CONSUMER, "linear.Issue.>").await?;

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
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let subject = "github.pull_request";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                            let autos = store.matching(subject, &pv).await.unwrap_or_default();
                            if autos.is_empty() {
                                let is_merged = pv["action"].as_str() == Some("closed")
                                    && pv["pull_request"]["merged"].as_bool() == Some(true);
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
                            } else {
                                dispatch_automations(&agent, autos, subject, &msg.payload).await;
                            }
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack PR message"); }
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
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let subject = "github.issue_comment";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                            let autos = store.matching(subject, &pv).await.unwrap_or_default();
                            if autos.is_empty() {
                                match handlers::comment_added::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "Comment-added done"),
                                    Some(Err(e)) => error!(error = %e, "Comment-added error"),
                                    None => {}
                                }
                            } else {
                                dispatch_automations(&agent, autos, subject, &msg.payload).await;
                            }
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack comment message"); }
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
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let subject = "github.push";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                            let autos = store.matching(subject, &pv).await.unwrap_or_default();
                            if autos.is_empty() {
                                match handlers::push_to_branch::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "Push-to-branch done"),
                                    Some(Err(e)) => error!(error = %e, "Push-to-branch error"),
                                    None => {}
                                }
                            } else {
                                dispatch_automations(&agent, autos, subject, &msg.payload).await;
                            }
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack push message"); }
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
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let subject = "github.check_run";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                            let autos = store.matching(subject, &pv).await.unwrap_or_default();
                            if autos.is_empty() {
                                match handlers::ci_completed::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "CI-completed done"),
                                    Some(Err(e)) => error!(error = %e, "CI-completed error"),
                                    None => {}
                                }
                            } else {
                                dispatch_automations(&agent, autos, subject, &msg.payload).await;
                            }
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack CI message"); }
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
                        let store = Arc::clone(&store);
                        tokio::spawn(async move {
                            let subject = "linear.Issue";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_default();
                            let autos = store.matching(subject, &pv).await.unwrap_or_default();
                            if autos.is_empty() {
                                match handlers::issue_triage::handle(&agent, &msg.payload).await {
                                    Some(Ok(o)) => info!(output = %o, "Issue triage done"),
                                    Some(Err(e)) => error!(error = %e, "Issue triage error"),
                                    None => {}
                                }
                            } else {
                                dispatch_automations(&agent, autos, subject, &msg.payload).await;
                            }
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack issue message"); }
                        });
                    }
                }
            }
        }
    }

    info!("Agent runner stopped");
    Ok(())
}

/// Spawn one task per automation and wait for all to finish.
async fn dispatch_automations(
    agent: &Arc<AgentLoop>,
    automations: Vec<trogon_automations::Automation>,
    nats_subject: &str,
    payload: &bytes::Bytes,
) {
    let handles: Vec<_> = automations
        .into_iter()
        .map(|auto| {
            let agent = Arc::clone(agent);
            let payload = payload.clone();
            let subject = nats_subject.to_string();
            tokio::spawn(async move {
                info!(automation = %auto.name, "Running automation");
                match handlers::run_automation(&agent, &auto, &subject, &payload).await {
                    Ok(o) => info!(automation = %auto.name, output = %o, "Automation done"),
                    Err(e) => error!(automation = %auto.name, error = %e, "Automation failed"),
                }
            })
        })
        .collect();

    for h in handles {
        h.await.ok();
    }
}

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
