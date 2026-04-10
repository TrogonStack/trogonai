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

use async_nats::jetstream::{
    self, AckKind, consumer::AckPolicy, consumer::DeliverPolicy, consumer::pull,
};
use futures_util::StreamExt;
use tracing::{error, info, warn};
use trogon_automations::{AutomationStore, RunRecord, RunStatus, RunStore};

use crate::agent_loop::{AgentLoop, ReqwestAnthropicClient};
use crate::chat_api::{ChatAppState, router as chat_router};
use crate::config::{AgentConfig, McpServerConfig};
use crate::flag_client::{AlwaysOnFlagClient, SplitFlagClient};
use crate::handlers::{self, make_tool_context};
use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus, PromiseStore};
use crate::session::SessionStore;
use crate::tools::{DefaultToolDispatcher, ToolContext, ToolDef};

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `ack_wait` for all consumers — set to 3× the heartbeat interval.
///
/// `spawn_heartbeat` sends `AckKind::Progress` every 15s, which resets the
/// ack timer on the server. Redelivery only triggers if the process actually
/// dies (the heartbeat stops). 45s = 3× 15s gives three missed beats before
/// NATS redelivers — enough to absorb transient GC pauses without being so
/// long that a real crash goes undetected for minutes.
const CONSUMER_ACK_WAIT: Duration = Duration::from_secs(45);

/// Worker identifier for promise ownership — hostname + PID.
fn worker_id() -> String {
    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
    let pid = std::process::id();
    format!("{hostname}:{pid}")
}

const GITHUB_CONSUMER: &str = "trogon-agent-pr-review";
const COMMENT_CONSUMER: &str = "trogon-agent-comment-added";
const PUSH_CONSUMER: &str = "trogon-agent-push-to-branch";
const CI_CONSUMER: &str = "trogon-agent-ci-completed";
const LINEAR_CONSUMER: &str = "trogon-agent-issue-triage";
const CRON_CONSUMER: &str = "trogon-agent-cron";
const DATADOG_CONSUMER: &str = "trogon-agent-datadog-alert";
const INCIDENTIO_CONSUMER: &str = "trogon-agent-incidentio";

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

    let tool_ctx: Arc<ToolContext<reqwest::Client>> = make_tool_context(
        http_client.clone(),
        cfg.proxy_url.clone(),
        cfg.github_token.clone(),
        cfg.linear_token.clone(),
        cfg.slack_token.clone(),
    );

    let split_client = cfg.split_evaluator_url.as_ref().map(|url| {
        trogon_splitio::SplitClient::new(trogon_splitio::SplitConfig {
            evaluator_url: url.clone(),
            auth_token: cfg.split_auth_token.clone().unwrap_or_default(),
        })
    });

    let (mcp_tool_defs, mcp_dispatch) = init_mcp_servers(&http_client, &cfg.mcp_servers).await;

    let anthropic_client: Arc<dyn crate::agent_loop::AnthropicClient> =
        Arc::new(ReqwestAnthropicClient::new(
            http_client.clone(),
            cfg.proxy_url.clone(),
            cfg.anthropic_token.clone(),
        ));

    let tool_dispatcher: Arc<dyn crate::tools::ToolDispatcher> =
        Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx)));

    let flag_client: Arc<dyn crate::flag_client::FeatureFlagClient> = match split_client {
        Some(c) => Arc::new(SplitFlagClient::new(c)),
        None => Arc::new(AlwaysOnFlagClient),
    };

    let agent = Arc::new(AgentLoop {
        anthropic_client,
        model: cfg.model.clone(),
        max_iterations: cfg.max_iterations,
        tool_dispatcher,
        tool_context: tool_ctx,
        memory_owner: cfg.memory_owner.clone(),
        memory_repo: cfg.memory_repo.clone(),
        memory_path: cfg.memory_path.clone(),
        mcp_tool_defs,
        mcp_dispatch,
        flag_client,
        tenant_id: cfg.tenant_id.clone(),
        // Promise fields are set per-run by `prepare_agent_with_promise`.
        promise_store: None,
        promise_id: None,
    });

    let store = Arc::new(
        AutomationStore::open(&js)
            .await
            .map_err(|e| RunnerError::JetStream(format!("AutomationStore: {e}")))?,
    );
    let run_store = Arc::new(
        RunStore::open(&js)
            .await
            .map_err(|e| RunnerError::JetStream(format!("RunStore: {e}")))?,
    );
    let session_store = SessionStore::open(&js)
        .await
        .map_err(|e| RunnerError::JetStream(format!("SessionStore: {e}")))?;
    let promise_store: Arc<dyn PromiseRepository> = Arc::new(
        PromiseStore::open(&js)
            .await
            .map_err(|e| RunnerError::JetStream(format!("PromiseStore: {e}")))?,
    );
    let tenant_id = Arc::new(cfg.tenant_id.clone());

    // Recover any stale promises from a previous process run.
    recover_stale_promises(&agent, &promise_store, &store, &run_store, &tenant_id).await;

    // Start the combined HTTP API server unless disabled (port == 0).
    // Automations + run history + interactive chat sessions are all on the same port.
    if cfg.api_port != 0 {
        let auto_state = trogon_automations::api::AppState {
            store: (*store).clone(),
            run_store: (*run_store).clone(),
        };
        let chat_state = ChatAppState {
            agent: Arc::clone(&agent),
            session_store,
        };
        let api_port = cfg.api_port;
        tokio::spawn(async move {
            let combined =
                trogon_automations::api::router(auto_state).merge(chat_router(chat_state));
            match tokio::net::TcpListener::bind(("0.0.0.0", api_port)).await {
                Err(e) => tracing::error!(port = api_port, error = %e, "Failed to bind API"),
                Ok(listener) => {
                    tracing::info!(port = api_port, "API listening (automations + chat)");
                    if let Err(e) = axum::serve(listener, combined).await {
                        tracing::error!(error = %e, "API server error");
                    }
                }
            }
        });
    }

    let github_stream_name = cfg.github_stream_name.as_deref().unwrap_or("GITHUB");
    let linear_stream_name = cfg.linear_stream_name.as_deref().unwrap_or("LINEAR");
    let cron_stream_name = cfg.cron_stream_name.as_deref().unwrap_or("CRON_TICKS");
    let datadog_stream_name = cfg.datadog_stream_name.as_deref().unwrap_or("DATADOG");

    // Ensure all required JetStream streams exist before binding consumers.
    // This removes the hard startup-order dependency on trogon-github, trogon-linear,
    // and trogon-cron — the agent creates streams idempotently if they are absent.
    ensure_stream(
        &js,
        github_stream_name,
        &["github.>"],
        Duration::from_secs(7 * 86_400),
    )
    .await?;
    ensure_stream(
        &js,
        linear_stream_name,
        &["linear.>"],
        Duration::from_secs(7 * 86_400),
    )
    .await?;
    ensure_stream(
        &js,
        cron_stream_name,
        &["cron.>"],
        Duration::from_secs(86_400),
    )
    .await?;
    ensure_stream(
        &js,
        datadog_stream_name,
        &["datadog.>"],
        Duration::from_secs(7 * 86_400),
    )
    .await?;

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
    let mut push_messages =
        bind_consumer(&js, github_stream_name, PUSH_CONSUMER, "github.push").await?;
    let mut ci_messages =
        bind_consumer(&js, github_stream_name, CI_CONSUMER, "github.check_run").await?;
    let mut issue_messages =
        bind_consumer(&js, linear_stream_name, LINEAR_CONSUMER, "linear.Issue.>").await?;
    let mut cron_messages = bind_consumer(&js, cron_stream_name, CRON_CONSUMER, "cron.>").await?;
    let mut datadog_messages =
        bind_consumer(&js, datadog_stream_name, DATADOG_CONSUMER, "datadog.>").await?;

    // incident.io stream is optional: when `incidentio_stream_name` is `None` the runner
    // skips both stream creation and consumer binding so no incidentio events are processed.
    let mut incidentio_messages_opt = match cfg.incidentio_stream_name.as_deref() {
        None => {
            info!("incidentio_stream_name is None — skipping incident.io subscription");
            None
        }
        Some(name) => {
            ensure_stream(
                &js,
                name,
                &["incidentio.>"],
                Duration::from_secs(7 * 86_400),
            )
            .await?;
            Some(bind_consumer(&js, name, INCIDENTIO_CONSUMER, "incidentio.>").await?)
        }
    };

    info!(
        proxy_url = %cfg.proxy_url,
        model = %cfg.model,
        github_stream = github_stream_name,
        linear_stream = linear_stream_name,
        datadog_stream = datadog_stream_name,
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
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        tokio::spawn(async move {
                            let subject = "github.pull_request";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("github.{stream_seq}");
                            let autos = match store.matching(&tenant_id, subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await;
                                let is_merged = pv["action"].as_str() == Some("closed")
                                    && pv["pull_request"]["merged"].as_bool() == Some(true);
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::PrReviewEnabled).await {
                                    info!(flag = "agent_pr_review_enabled", "PR handler disabled by feature flag");
                                } else if is_merged {
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
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, subject, &msg.payload).await;
                            }
                            heartbeat.abort();
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
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        tokio::spawn(async move {
                            let subject = "github.issue_comment";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("github.{stream_seq}");
                            let autos = match store.matching(&tenant_id, subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::CommentHandlerEnabled).await {
                                    info!(flag = "agent_comment_handler_enabled", "Comment handler disabled by feature flag");
                                } else {
                                    match handlers::comment_added::handle(&agent, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "Comment-added done"),
                                        Some(Err(e)) => error!(error = %e, "Comment-added error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, subject, &msg.payload).await;
                            }
                            heartbeat.abort();
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
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        tokio::spawn(async move {
                            let subject = "github.push";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("github.{stream_seq}");
                            let autos = match store.matching(&tenant_id, subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::PushHandlerEnabled).await {
                                    info!(flag = "agent_push_handler_enabled", "Push handler disabled by feature flag");
                                } else {
                                    match handlers::push_to_branch::handle(&agent, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "Push-to-branch done"),
                                        Some(Err(e)) => error!(error = %e, "Push-to-branch error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, subject, &msg.payload).await;
                            }
                            heartbeat.abort();
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
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        tokio::spawn(async move {
                            let subject = "github.check_run";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("github.{stream_seq}");
                            let autos = match store.matching(&tenant_id, subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::CiHandlerEnabled).await {
                                    info!(flag = "agent_ci_handler_enabled", "CI handler disabled by feature flag");
                                } else {
                                    match handlers::ci_completed::handle(&agent, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "CI-completed done"),
                                        Some(Err(e)) => error!(error = %e, "CI-completed error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, subject, &msg.payload).await;
                            }
                            heartbeat.abort();
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
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        tokio::spawn(async move {
                            let subject = "linear.Issue";
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("linear.{stream_seq}");
                            let autos = match store.matching(&tenant_id, subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::IssueTriageEnabled).await {
                                    info!(flag = "agent_issue_triage_enabled", "Issue triage handler disabled by feature flag");
                                } else {
                                    match handlers::issue_triage::handle(&agent, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "Issue triage done"),
                                        Some(Err(e)) => error!(error = %e, "Issue triage error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, subject, &msg.payload).await;
                            }
                            heartbeat.abort();
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack issue message"); }
                        });
                    }
                }
            }
            msg = cron_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving cron message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        let store = Arc::clone(&store);
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        let nats_subject = msg.subject.to_string();
                        tokio::spawn(async move {
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("cron.{stream_seq}");
                            let autos = match store.matching(&tenant_id, &nats_subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject = %nats_subject, "Automation lookup failed — cron tick skipped");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                info!(subject = %nats_subject, "Cron tick with no matching automations — skipping");
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, &nats_subject, &msg.payload).await;
                            }
                            heartbeat.abort();
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack cron message"); }
                        });
                    }
                }
            }
            msg = datadog_messages.next() => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving Datadog message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        let store = Arc::clone(&store);
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        let nats_subject = msg.subject.to_string();
                        tokio::spawn(async move {
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("datadog.{stream_seq}");
                            let autos = match store.matching(&tenant_id, &nats_subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject = %nats_subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", &nats_subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::AlertHandlerEnabled).await {
                                    info!(flag = "agent_alert_handler_enabled", "Alert handler disabled by feature flag");
                                } else {
                                    match handlers::alert_triggered::handle(&agent, &nats_subject, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "Datadog alert handled"),
                                        Some(Err(e)) => error!(error = %e, "Datadog alert handler error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, &nats_subject, &msg.payload).await;
                            }
                            heartbeat.abort();
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack Datadog message"); }
                        });
                    }
                }
            }
            msg = async {
                match incidentio_messages_opt.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(msg) = msg else { break };
                match msg {
                    Err(e) => warn!(error = %e, "Error receiving incident.io message"),
                    Ok(msg) => {
                        let agent = Arc::clone(&agent);
                        let store = Arc::clone(&store);
                        let run_store = Arc::clone(&run_store);
                        let promise_store = Arc::clone(&promise_store);
                        let tenant_id = Arc::clone(&tenant_id);
                        let nats_subject = msg.subject.to_string();
                        tokio::spawn(async move {
                            let pv: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap_or_else(|e| {
                                warn!(error = %e, "Failed to parse NATS message payload as JSON — processing with empty value");
                                serde_json::Value::default()
                            });
                            let stream_seq = match msg.info() {
                                Ok(info) => info.stream_sequence,
                                Err(_) => {
                                    error!("JetStream message missing sequence info — promise_id cannot be derived, acking to prevent infinite redelivery");
                                    let _ = msg.ack().await;
                                    return;
                                }
                            };
                            let promise_id_prefix = format!("incidentio.{stream_seq}");
                            let autos = match store.matching(&tenant_id, &nats_subject, &pv).await {
                                Ok(list) => list,
                                Err(e) => {
                                    error!(error = %e, subject = %nats_subject, "Automation lookup failed — falling back to built-in handler");
                                    vec![]
                                }
                            };
                            let heartbeat = spawn_heartbeat(msg.clone());
                            if autos.is_empty() {
                                let agent = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", &nats_subject, &pv).await;
                                if !agent.is_flag_enabled(&crate::flags::AgentFlag::IncidentioHandlerEnabled).await {
                                    info!(flag = "agent_incidentio_handler_enabled", "incident.io handler disabled by feature flag");
                                } else {
                                    match handlers::incident_declared::handle(&agent, &nats_subject, &msg.payload).await {
                                        Some(Ok(o)) => info!(output = %o, "incident.io event handled"),
                                        Some(Err(e)) => error!(error = %e, "incident.io handler error"),
                                        None => {}
                                    }
                                }
                            } else {
                                dispatch_automations(&agent, &run_store, &promise_store, &promise_id_prefix, autos, &nats_subject, &msg.payload).await;
                            }
                            heartbeat.abort();
                            if let Err(e) = msg.ack().await { warn!(error = %e, "Failed to ack incident.io message"); }
                        });
                    }
                }
            }
        }
    }

    info!("Agent runner stopped");
    Ok(())
}

/// Create or find an existing promise for a run, then return a clone of `agent`
/// with `promise_store` and `promise_id` set so the agentic loop can checkpoint.
///
/// If a promise already exists in KV (from a previous attempt on the same
/// redelivered NATS message), the agent will resume from its checkpointed state
/// when it calls `run()`. If no promise exists, a new one is created.
async fn prepare_agent_with_promise(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    tenant_id: &str,
    promise_id: &str,
    automation_id: &str,
    nats_subject: &str,
    trigger: &serde_json::Value,
) -> Arc<AgentLoop> {
    let wid = worker_id();
    match promise_store.get_promise(tenant_id, promise_id).await {
        Ok(Some((existing, _))) => {
            info!(
                promise_id = %promise_id,
                status = ?existing.status,
                iteration = existing.iteration,
                "Found existing promise — resuming"
            );
        }
        Ok(None) => {
            let promise = AgentPromise {
                id: promise_id.to_string(),
                tenant_id: tenant_id.to_string(),
                automation_id: automation_id.to_string(),
                status: PromiseStatus::Running,
                messages: vec![],
                iteration: 0,
                worker_id: wid,
                claimed_at: trogon_automations::now_unix(),
                trigger: trigger.clone(),
                nats_subject: nats_subject.to_string(),
            };
            if let Err(e) = promise_store.put_promise(&promise).await {
                warn!(promise_id = %promise_id, error = %e, "Failed to create promise");
            }
        }
        Err(e) => {
            warn!(promise_id = %promise_id, error = %e, "Failed to check promise — continuing without durability");
            // Fall through: run without promise fields so the agent works as before.
            return Arc::clone(agent);
        }
    }

    let mut agent_with_promise = (**agent).clone();
    agent_with_promise.promise_store = Some(Arc::clone(promise_store));
    agent_with_promise.promise_id = Some(promise_id.to_string());
    Arc::new(agent_with_promise)
}

/// On startup, resume any agent runs that were left in `Running` state by a
/// previous process instance that crashed before completing.
///
/// Only recovers promises that are owned by this tenant and have been running
/// for more than `stale_after` seconds without completing — indicating the
/// original process is gone.
async fn recover_stale_promises(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    automation_store: &Arc<AutomationStore>,
    run_store: &Arc<RunStore>,
    tenant_id: &str,
) {
    const STALE_AFTER_SECS: u64 = 5 * 60;
    /// Maximum number of stale promises to recover on a single startup.
    ///
    /// Each recovery re-runs the full agent (potentially multiple LLM calls),
    /// so recovering an unbounded number sequentially would delay fresh event
    /// processing when many promises are stale (e.g. after a long outage).
    /// Cap at 10 most-recent by claimed_at; the rest stay Running and are
    /// picked up on the next restart once the backlog has drained.
    const MAX_RECOVERY_AT_STARTUP: usize = 10;

    let now = trogon_automations::now_unix();

    let promises = match promise_store.list_running(tenant_id).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Startup recovery: failed to list running promises");
            return;
        }
    };

    let mut stale: Vec<_> = promises
        .into_iter()
        .filter(|p| now.saturating_sub(p.claimed_at) >= STALE_AFTER_SECS)
        .collect();

    if stale.is_empty() {
        return;
    }

    // Recover the most recently active first (those closest to completion
    // are most likely to succeed quickly). Truncate to cap.
    stale.sort_by(|a, b| b.claimed_at.cmp(&a.claimed_at));
    if stale.len() > MAX_RECOVERY_AT_STARTUP {
        warn!(
            total = stale.len(),
            recovering = MAX_RECOVERY_AT_STARTUP,
            "Startup recovery: capping to {} most recent stale promises — {} deferred to next restart",
            MAX_RECOVERY_AT_STARTUP,
            stale.len() - MAX_RECOVERY_AT_STARTUP,
        );
        stale.truncate(MAX_RECOVERY_AT_STARTUP);
    }

    // Recovery runs in the background so the consumer loop can start accepting
    // new messages immediately. Each stale promise is resumed sequentially to
    // avoid hammering the Anthropic API with concurrent runs.
    info!(
        count = stale.len(),
        "Startup recovery: resuming stale promises in background"
    );

    let agent = Arc::clone(agent);
    let promise_store = Arc::clone(promise_store);
    let automation_store = Arc::clone(automation_store);
    let run_store = Arc::clone(run_store);
    let tenant_id = tenant_id.to_string();

    tokio::spawn(async move {
        for promise in stale {
            // Re-fetch to get the current revision and verify the promise is
            // still Running. Between `list_running` and here, another worker
            // (concurrent restart) may have already claimed or completed it.
            let rev = match promise_store.get_promise(&tenant_id, &promise.id).await {
                Ok(Some((current, rev))) => {
                    if current.status != PromiseStatus::Running {
                        info!(
                            promise_id = %promise.id,
                            status = ?current.status,
                            "Startup recovery: promise already completed, skipping"
                        );
                        continue;
                    }
                    rev
                }
                Ok(None) => {
                    info!(
                        promise_id = %promise.id,
                        "Startup recovery: promise vanished before claim, skipping"
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        promise_id = %promise.id,
                        error = %e,
                        "Startup recovery: failed to fetch promise for CAS claim, skipping"
                    );
                    continue;
                }
            };

            // CAS-claim: update worker_id + claimed_at under the current revision.
            // If two workers race, only one wins the CAS write; the other skips.
            let mut claimed = promise.clone();
            claimed.worker_id = worker_id();
            claimed.claimed_at = trogon_automations::now_unix();
            if let Err(e) = promise_store
                .update_promise(&tenant_id, &promise.id, &claimed, rev)
                .await
            {
                info!(
                    promise_id = %promise.id,
                    error = %e,
                    "Startup recovery: CAS claim lost — another worker took this promise"
                );
                continue;
            }

            info!(
                promise_id = %promise.id,
                subject = %promise.nats_subject,
                iteration = promise.iteration,
                "Startup recovery: resuming stale promise"
            );

            let payload: bytes::Bytes = serde_json::to_vec(&promise.trigger)
                .unwrap_or_default()
                .into();

            let agent = prepare_agent_with_promise(
                &agent,
                &promise_store,
                &tenant_id,
                &promise.id,
                &promise.automation_id,
                &promise.nats_subject,
                &promise.trigger,
            )
            .await;

            if !promise.automation_id.is_empty() {
                // Automation run
                //
                // Propagate list() errors instead of using unwrap_or_default().
                // An empty vec from a transient failure looks identical to "no
                // automations exist", which would cause the else branch below to
                // permanently mark the promise as Failed — losing valid work.
                // On error we skip this promise; it stays Running and will be
                // picked up on the next restart once the store is available.
                let autos = match automation_store.list(&tenant_id).await {
                    Ok(list) => list,
                    Err(e) => {
                        error!(
                            error = %e,
                            promise_id = %promise.id,
                            automation_id = %promise.automation_id,
                            "Startup recovery: failed to list automations — skipping promise, will retry on next restart"
                        );
                        continue;
                    }
                };
                if let Some(auto) = autos.into_iter().find(|a| a.id == promise.automation_id) {
                    let started_at = trogon_automations::now_unix();
                    let result =
                        handlers::run_automation(&agent, &auto, &promise.nats_subject, &payload)
                            .await;
                    let finished_at = trogon_automations::now_unix();
                    let (status, output) = match &result {
                        Ok(o) => (RunStatus::Success, o.clone()),
                        Err(e) => (RunStatus::Failed, e.clone()),
                    };
                    let run = RunRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        automation_id: auto.id.clone(),
                        automation_name: auto.name.clone(),
                        tenant_id: tenant_id.clone(),
                        nats_subject: promise.nats_subject.clone(),
                        started_at,
                        finished_at,
                        status,
                        output,
                    };
                    if let Err(e) = run_store.record(&run).await {
                        warn!(error = %e, "Startup recovery: failed to persist run record");
                    }
                } else {
                    // Automation was deleted between the crash and this recovery.
                    // Mark the promise as Failed so it is not picked up on future
                    // restarts — a Running promise with no active worker is noise.
                    warn!(
                        promise_id = %promise.id,
                        automation_id = %promise.automation_id,
                        "Startup recovery: automation no longer exists — marking promise as Failed"
                    );
                    if let Ok(Some((mut current, rev))) =
                        promise_store.get_promise(&tenant_id, &promise.id).await
                    {
                        current.status = PromiseStatus::Failed;
                        if let Err(e) = promise_store
                            .update_promise(&tenant_id, &promise.id, &current, rev)
                            .await
                        {
                            warn!(
                                promise_id = %promise.id,
                                error = %e,
                                "Startup recovery: failed to mark orphaned promise as Failed"
                            );
                        }
                    }
                }
            } else {
                // Built-in handler dispatch based on NATS subject
                match promise.nats_subject.as_str() {
                    s if s.starts_with("github.pull_request") => {
                        handlers::pr_review::handle(&agent, &payload).await;
                    }
                    s if s.starts_with("github.check_run") => {
                        handlers::ci_completed::handle(&agent, &payload).await;
                    }
                    s if s.starts_with("github.push") => {
                        handlers::push_to_branch::handle(&agent, &payload).await;
                    }
                    s if s.starts_with("github.issue_comment") => {
                        handlers::comment_added::handle(&agent, &payload).await;
                    }
                    s if s.starts_with("linear.") => {
                        handlers::issue_triage::handle(&agent, &payload).await;
                    }
                    other => {
                        warn!(subject = %other, "Startup recovery: unknown subject, skipping");
                    }
                }
            }
        }
    });
}

/// Spawn a background task that sends [`AckKind::Progress`] every 15 seconds
/// to prevent JetStream from redelivering the message while the agent is active.
///
/// The returned handle **must** be aborted once processing finishes so the task
/// stops sending progress acks after the message has been explicitly acked.
fn spawn_heartbeat(msg: async_nats::jetstream::Message) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            if msg.ack_with(AckKind::Progress).await.is_err() {
                break;
            }
        }
    })
}

/// Spawn one task per automation and wait for all to finish.
/// Persists a [`RunRecord`] in `run_store` after each execution.
pub(crate) async fn dispatch_automations(
    agent: &Arc<AgentLoop>,
    run_store: &Arc<RunStore>,
    promise_store: &Arc<dyn PromiseRepository>,
    promise_id_prefix: &str,
    automations: Vec<trogon_automations::Automation>,
    nats_subject: &str,
    payload: &bytes::Bytes,
) {
    let handles: Vec<_> = automations
        .into_iter()
        .map(|auto| {
            let agent = Arc::clone(agent);
            let run_store = Arc::clone(run_store);
            let promise_store = Arc::clone(promise_store);
            let payload = payload.clone();
            let subject = nats_subject.to_string();
            let trigger: serde_json::Value = serde_json::from_slice(&payload).unwrap_or_default();
            let promise_id_prefix = promise_id_prefix.to_string();
            tokio::spawn(async move {
                // Each automation gets its own promise so concurrent automations
                // triggered by the same NATS message checkpoint independently.
                // KV key = {tenant_id}.{promise_id} so promise_id must NOT
                // include tenant_id again. The prefix encodes the stream name
                // (e.g. "github.42") to avoid collisions with same-seq events
                // from different streams (GITHUB, LINEAR, CRON, DATADOG each
                // have independent sequence counters starting from 1).
                let promise_id = format!("{promise_id_prefix}.{}", auto.id);
                let tenant_id = agent.tenant_id.clone();
                let agent = prepare_agent_with_promise(
                    &agent,
                    &promise_store,
                    &tenant_id,
                    &promise_id,
                    &auto.id,
                    &subject,
                    &trigger,
                )
                .await;

                info!(automation = %auto.name, "Running automation");
                let started_at = trogon_automations::now_unix();
                let result = handlers::run_automation(&agent, &auto, &subject, &payload).await;
                let finished_at = trogon_automations::now_unix();

                let (status, output) = match &result {
                    Ok(o) => {
                        info!(automation = %auto.name, output = %o, "Automation done");
                        (RunStatus::Success, o.clone())
                    }
                    Err(e) => {
                        error!(automation = %auto.name, error = %e, "Automation failed");
                        (RunStatus::Failed, e.clone())
                    }
                };

                let run = RunRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    automation_id: auto.id.clone(),
                    automation_name: auto.name.clone(),
                    tenant_id: auto.tenant_id.clone(),
                    nats_subject: subject,
                    started_at,
                    finished_at,
                    status,
                    output,
                };
                if let Err(e) = run_store.record(&run).await {
                    warn!(automation = %auto.name, error = %e, "Failed to persist run record");
                }
            })
        })
        .collect();

    for h in handles {
        if let Err(e) = h.await {
            if e.is_panic() {
                error!(error = ?e, "Automation task panicked");
            } else {
                warn!(error = ?e, "Automation task was cancelled");
            }
        }
    }
}

pub async fn init_mcp_servers(
    http_client: &reqwest::Client,
    servers: &[McpServerConfig],
) -> (
    Vec<ToolDef>,
    Vec<(String, String, Arc<dyn trogon_mcp::McpCallTool>)>,
) {
    let mut tool_defs = Vec::new();
    let mut dispatch = Vec::new();

    for server in servers {
        let client = Arc::new(trogon_mcp::McpClient::new(http_client.clone(), &server.url));

        if let Err(e) = client.initialize().await {
            warn!(server = %server.name, error = %e, "Failed to initialize MCP server — skipping");
            continue;
        }

        match client.list_tools().await {
            Err(e) => {
                warn!(server = %server.name, error = %e, "Failed to list MCP tools — skipping server")
            }
            Ok(tools) => {
                info!(server = %server.name, count = tools.len(), "MCP tools loaded");
                for tool in tools {
                    let prefixed = format!(
                        "mcp__{}__{}",
                        sanitize_name(&server.name),
                        sanitize_name(&tool.name)
                    );
                    tool_defs.push(ToolDef {
                        name: prefixed.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                        cache_control: None,
                    });
                    dispatch.push((
                        prefixed,
                        tool.name,
                        Arc::clone(&client) as Arc<dyn trogon_mcp::McpCallTool>,
                    ));
                }
            }
        }
    }

    (tool_defs, dispatch)
}

pub(crate) fn sanitize_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn ensure_stream(
    js: &jetstream::Context,
    stream_name: &str,
    subjects: &[&str],
    max_age: Duration,
) -> Result<(), RunnerError> {
    let result = js
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.to_string(),
            subjects: subjects.iter().map(|s| s.to_string()).collect(),
            max_age,
            ..Default::default()
        })
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            // If the stream already exists with a different config (e.g. narrower subjects),
            // fall back to using the existing stream rather than failing.
            if js.get_stream(stream_name).await.is_ok() {
                tracing::debug!(stream = stream_name, error = %e, "Stream exists with different config — using as-is");
                Ok(())
            } else {
                Err(RunnerError::JetStream(format!(
                    "ensure stream {stream_name}: {e}"
                )))
            }
        }
    }
}

async fn bind_consumer(
    js: &jetstream::Context,
    stream_name: &str,
    consumer_name: &str,
    filter_subject: &str,
) -> Result<
    impl futures_util::Stream<
        Item = Result<
            jetstream::Message,
            async_nats::error::Error<jetstream::consumer::pull::MessagesErrorKind>,
        >,
    >,
    RunnerError,
> {
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
                ack_wait: CONSUMER_ACK_WAIT,
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

#[cfg(test)]
mod tests {
    use super::{dispatch_automations, sanitize_name};
    use std::sync::Arc;

    fn make_agent(proxy_url: &str) -> Arc<crate::agent_loop::AgentLoop> {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext::for_test(
            proxy_url,
            "tok_github_prod_test01",
            "",
            "",
        ));
        Arc::new(crate::agent_loop::AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http_client,
                proxy_url.to_string(),
                String::new(),
            )),
            model: "test".to_string(),
            max_iterations: 1,
            tool_dispatcher: Arc::new(DefaultToolDispatcher::new(Arc::clone(&tool_ctx))),
            tool_context: tool_ctx,
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            flag_client: Arc::new(AlwaysOnFlagClient),
            tenant_id: "test".to_string(),
            promise_store: None,
            promise_id: None,
        })
    }

    fn make_automation(name: &str) -> trogon_automations::Automation {
        trogon_automations::Automation {
            id: format!("id-{name}"),
            tenant_id: "acme".to_string(),
            name: name.to_string(),
            trigger: "github.push".to_string(),
            prompt: "Do something.".to_string(),
            model: None,
            tools: vec![],
            memory_path: None,
            mcp_servers: vec![],
            enabled: true,
            visibility: trogon_automations::Visibility::Private,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    async fn make_run_store() -> (trogon_automations::RunStore, impl Drop) {
        use testcontainers_modules::{
            nats::Nats,
            testcontainers::{ImageExt, runners::AsyncRunner},
        };
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("NATS");
        let port = container.get_host_port_ipv4(4222).await.expect("port");
        let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("connect");
        let js = async_nats::jetstream::new(nats);
        let rs = trogon_automations::RunStore::open(&js)
            .await
            .expect("RunStore");
        (rs, container)
    }

    /// dispatch_automations runs every automation and waits for all tasks.
    #[tokio::test]
    async fn dispatch_automations_runs_all() {
        use crate::promise_store::mock::MockPromiseStore;

        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "ok"}]
                }));
        });

        let (rs, _container) = make_run_store().await;
        let agent = make_agent(&server.base_url());
        let automations = vec![make_automation("auto-1"), make_automation("auto-2")];
        let payload = bytes::Bytes::from_static(b"{}");
        let promise_store: Arc<dyn crate::promise_store::PromiseRepository> =
            Arc::new(MockPromiseStore::new());

        dispatch_automations(
            &agent,
            &Arc::new(rs),
            &promise_store,
            "github.42",
            automations,
            "github.push",
            &payload,
        )
        .await;

        mock.assert_hits_async(2).await;
    }

    /// dispatch_automations with an empty list completes immediately without errors.
    #[tokio::test]
    async fn dispatch_automations_empty_list_is_noop() {
        use crate::promise_store::mock::MockPromiseStore;

        let (rs, _container) = make_run_store().await;
        let agent = make_agent("http://127.0.0.1:1");
        let payload = bytes::Bytes::from_static(b"{}");
        let promise_store: Arc<dyn crate::promise_store::PromiseRepository> =
            Arc::new(MockPromiseStore::new());

        dispatch_automations(
            &agent,
            &Arc::new(rs),
            &promise_store,
            "github.0",
            vec![],
            "github.push",
            &payload,
        )
        .await;
    }

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

    /// An MCP server that fails `initialize` is skipped — no tools, no panic.
    #[tokio::test]
    async fn init_mcp_servers_skips_server_that_fails_initialize() {
        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .body_contains("\"initialize\"");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "error": { "code": -32600, "message": "not supported" }
                }));
        });

        let cfg = vec![crate::config::McpServerConfig {
            name: "bad-server".to_string(),
            url: format!("{}/mcp", server.base_url()),
        }];
        let http_client = reqwest::Client::new();
        let (tools, dispatch) = super::init_mcp_servers(&http_client, &cfg).await;

        assert!(
            tools.is_empty(),
            "no tools expected when server fails to initialize"
        );
        assert!(dispatch.is_empty(), "no dispatch entries expected");
    }

    /// An MCP server that initialises successfully but then fails `list_tools` is also skipped.
    #[tokio::test]
    async fn init_mcp_servers_skips_server_that_fails_list_tools() {
        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST).body_contains("\"initialize\"");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": { "protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {} }
                }));
        });
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .body_contains("tools/list");
            then.status(500);
        });

        let cfg = vec![crate::config::McpServerConfig {
            name: "flaky-server".to_string(),
            url: format!("{}/mcp", server.base_url()),
        }];
        let http_client = reqwest::Client::new();
        let (tools, dispatch) = super::init_mcp_servers(&http_client, &cfg).await;

        assert!(tools.is_empty(), "no tools expected when list_tools fails");
        assert!(dispatch.is_empty(), "no dispatch entries expected");
    }
}
