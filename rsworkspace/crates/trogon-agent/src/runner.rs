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
use trogon_automations::{AutomationRepository, AutomationStore, RunRecord, RunRepository, RunStatus, RunStore};

use crate::agent_loop::{AgentLoop, ReqwestAnthropicClient};
use crate::chat_api::{ChatAppState, router as chat_router};
use crate::config::{AgentConfig, McpServerConfig};
use crate::flag_client::{AlwaysOnFlagClient, SplitFlagClient};
use crate::handlers::{self, make_tool_context};
use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus, PromiseStore};
use crate::session::SessionStore;
use crate::tools::{DefaultToolDispatcher, ToolContext, ToolDef};

const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often `spawn_heartbeat` sends `AckKind::Progress` to reset the
/// JetStream ack timer. Changing this value automatically adjusts
/// [`CONSUMER_ACK_WAIT`] — both must stay in sync so the server never
/// redelivers while a live heartbeat is running.
const HEARTBEAT_INTERVAL_SECS: u64 = 15;

/// `ack_wait` for all consumers — exactly 3× [`HEARTBEAT_INTERVAL_SECS`].
///
/// Three missed beats before NATS redelivers: enough to absorb transient
/// GC pauses without being so long that a real crash goes undetected for
/// minutes. Derived from the constant so the relationship is
/// compiler-enforced — changing the heartbeat interval automatically keeps
/// this in sync.
const CONSUMER_ACK_WAIT: Duration = Duration::from_secs(HEARTBEAT_INTERVAL_SECS * 3);

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
    let _ = recover_stale_promises(&agent, &promise_store, &store, &run_store, &tenant_id).await;

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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::PrReviewEnabled).await {
                                        info!(flag = "agent_pr_review_enabled", "PR handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::CommentHandlerEnabled).await {
                                        info!(flag = "agent_comment_handler_enabled", "Comment handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::PushHandlerEnabled).await {
                                        info!(flag = "agent_push_handler_enabled", "Push handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::CiHandlerEnabled).await {
                                        info!(flag = "agent_ci_handler_enabled", "CI handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::IssueTriageEnabled).await {
                                        info!(flag = "agent_issue_triage_enabled", "Issue triage handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::AlertHandlerEnabled).await {
                                        info!(flag = "agent_alert_handler_enabled", "Alert handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", &nats_subject, &pv).await else { break 'handler; };
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
                                'handler: {
                                    if !agent.is_flag_enabled(&crate::flags::AgentFlag::IncidentioHandlerEnabled).await {
                                        info!(flag = "agent_incidentio_handler_enabled", "incident.io handler disabled by feature flag — skipping");
                                        break 'handler;
                                    }
                                    let Some(agent) = prepare_agent_with_promise(&agent, &promise_store, &tenant_id, &promise_id_prefix, "", &nats_subject, &pv).await else { break 'handler; };
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
/// Returns `None` when another worker already owns this promise — the caller
/// should skip running the agent entirely. This prevents concurrent execution
/// when startup recovery and a NATS redelivery arrive for the same promise at
/// the same time.
///
/// ## Ownership (CAS-claim)
///
/// When a Running promise already exists, this function CAS-claims it by
/// updating `worker_id` + `claimed_at` under the current KV revision. Only one
/// concurrent caller wins the CAS write; the other receives `None`.
///
/// `Resolved` and `PermanentFailed` promises are never CAS-claimed — `run()`
/// returns early for both without executing any LLM call. `Failed` promises ARE
/// CAS-claimed: their failure may have been transient (e.g. HTTP error) and NATS
/// redelivery is a legitimate retry opportunity.
async fn prepare_agent_with_promise(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    tenant_id: &str,
    promise_id: &str,
    automation_id: &str,
    nats_subject: &str,
    trigger: &serde_json::Value,
) -> Option<Arc<AgentLoop>> {
    let wid = worker_id();
    let get_result = match tokio::time::timeout(
        crate::agent_loop::NATS_KV_TIMEOUT,
        promise_store.get_promise(tenant_id, promise_id),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => {
            warn!(promise_id = %promise_id, "NATS KV get_promise timed out — continuing without durability");
            return Some(Arc::clone(agent));
        }
    };
    match get_result {
        Ok(Some((existing, rev))) => {
            if existing.status == PromiseStatus::Running
                || existing.status == PromiseStatus::Failed
            {
                // CAS-claim: take ownership so no other worker runs this
                // promise concurrently. `Running` promises are claimed on both
                // the normal NATS dispatch path and startup recovery.
                // `Failed` promises (transient errors — currently only HTTP)
                // are also re-claimed: a crash in the narrow window between
                // marking Failed and calling msg.ack() causes NATS redelivery,
                // and re-claiming ensures at most one worker retries the run.
                // `PermanentFailed` and `Resolved` return `None` directly
                // from this function — the caller skips the handler entirely,
                // aborts the heartbeat, and calls msg.ack(). `run()` is never
                // reached for terminal states.
                // Note: startup recovery only scans `Running` promises via
                // `list_running`, so `Failed` promises are only ever re-claimed
                // by the NATS redelivery path, not by startup recovery.
                let mut claimed = existing.clone();
                claimed.status = PromiseStatus::Running; // re-activate so list_running finds it if we crash mid-run
                claimed.worker_id = wid;
                claimed.claimed_at = trogon_automations::now_unix();
                let claim_result = match tokio::time::timeout(
                    crate::agent_loop::NATS_KV_TIMEOUT,
                    promise_store.update_promise(tenant_id, promise_id, &claimed, rev),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        warn!(promise_id = %promise_id, "NATS KV update_promise timed out during CAS claim — treating as claim lost, skipping");
                        return None;
                    }
                };
                if let Err(e) = claim_result {
                    // Could be a CAS revision conflict (another worker claimed
                    // first — benign) or a NATS error (bucket unavailable).
                    // The `error` field carries the actual cause; the action is
                    // the same either way: skip this run.
                    warn!(
                        promise_id = %promise_id,
                        error = %e,
                        "Promise claim failed — skipping to avoid concurrent execution"
                    );
                    return None;
                }
            }
            match existing.status {
                PromiseStatus::Running | PromiseStatus::Failed => {
                    info!(
                        promise_id = %promise_id,
                        status = ?existing.status,
                        iteration = existing.iteration,
                        "Found existing promise — resuming"
                    );
                }
                _ => {
                    // Resolved / PermanentFailed: the run already completed.
                    // NATS is redelivering because msg.ack() failed after the
                    // run finished. Return None so the caller skips the handler
                    // entirely — no memory.md fetch, no LLM call, no misleading
                    // "done" log. The caller's 'handler: block breaks on None,
                    // the heartbeat is aborted, and msg.ack() is called.
                    info!(
                        promise_id = %promise_id,
                        status = ?existing.status,
                        "Found existing promise — already terminal, acking without re-running"
                    );
                    return None;
                }
            }
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
                system_prompt: None, // populated at first checkpoint in agent_loop::run
            };
            match tokio::time::timeout(
                crate::agent_loop::NATS_KV_TIMEOUT,
                promise_store.put_promise(&promise),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(promise_id = %promise_id, error = %e, "Failed to create promise — run will proceed without durability"),
                Err(_) => warn!(promise_id = %promise_id, "NATS KV put_promise timed out — run will proceed without durability"),
            }
        }
        Err(e) => {
            warn!(promise_id = %promise_id, error = %e, "Failed to check promise — continuing without durability");
            // Fall through: run without promise fields so the agent works as before.
            return Some(Arc::clone(agent));
        }
    }

    let mut agent_with_promise = (**agent).clone();
    agent_with_promise.promise_store = Some(Arc::clone(promise_store));
    agent_with_promise.promise_id = Some(promise_id.to_string());
    Some(Arc::new(agent_with_promise))
}

/// On startup, resume any agent runs that were left in `Running` state by a
/// previous process instance that crashed before completing.
///
/// Only recovers promises that are owned by this tenant and have been running
/// for more than `stale_after` seconds without completing — indicating the
/// original process is gone.
async fn recover_stale_promises<A: AutomationRepository, R: RunRepository>(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    automation_store: &Arc<A>,
    run_store: &Arc<R>,
    tenant_id: &str,
) -> Option<tokio::task::JoinHandle<()>> {
    // Must be strictly greater than the maximum time between two consecutive
    // checkpoints. Between checkpoints, claimed_at does not update, so the
    // age can grow by up to (tool_execution_time + LLM_call_time). The LLM
    // hard timeout is 5 * 60 = 300 s — using the same value as STALE_AFTER_SECS
    // would give zero margin. 10 minutes gives a 2× buffer so a worst-case
    // slow LLM call never triggers a spurious startup recovery.
    const STALE_AFTER_SECS: u64 = 10 * 60;
    /// Maximum number of stale promises to recover on a single startup.
    ///
    /// Each recovery re-runs the full agent (potentially multiple LLM calls),
    /// so recovering an unbounded number sequentially would delay fresh event
    /// processing when many promises are stale (e.g. after a long outage).
    /// Cap at 10 most-recent by claimed_at; the rest stay Running and are
    /// picked up on the next restart once the backlog has drained.
    const MAX_RECOVERY_AT_STARTUP: usize = 10;

    let now = trogon_automations::now_unix();

    let promises = match tokio::time::timeout(
        crate::agent_loop::NATS_KV_TIMEOUT,
        promise_store.list_running(tenant_id),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            warn!(error = %e, "Startup recovery: failed to list running promises");
            return None;
        }
        Err(_) => {
            warn!("Startup recovery: list_running timed out — skipping recovery this startup");
            return None;
        }
    };

    let mut stale: Vec<_> = promises
        .into_iter()
        .filter(|p| now.saturating_sub(p.claimed_at) >= STALE_AFTER_SECS)
        .collect();

    if stale.is_empty() {
        return None;
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

    // Intentionally sequential: spawning all recoveries concurrently would
    // flood the Anthropic API with simultaneous requests after an outage when
    // many promises are stale. Sequential execution keeps the load profile
    // predictable and avoids triggering rate limits.
    Some(tokio::spawn(async move {
        for promise in stale {
            // Re-fetch to verify the promise is still Running. Between
            // `list_running` and here, another worker may have already claimed
            // or completed it. We don't CAS-claim here — prepare_agent_with_promise
            // handles claiming so both paths share the same mechanism.
            match tokio::time::timeout(
                crate::agent_loop::NATS_KV_TIMEOUT,
                promise_store.get_promise(&tenant_id, &promise.id),
            )
            .await
            {
                Ok(Ok(Some((current, _)))) => {
                    if current.status != PromiseStatus::Running {
                        info!(
                            promise_id = %promise.id,
                            status = ?current.status,
                            "Startup recovery: promise already completed, skipping"
                        );
                        continue;
                    }
                    // Running — fall through to prepare_agent_with_promise
                }
                Ok(Ok(None)) => {
                    info!(
                        promise_id = %promise.id,
                        "Startup recovery: promise vanished before claim, skipping"
                    );
                    continue;
                }
                Ok(Err(e)) => {
                    warn!(
                        promise_id = %promise.id,
                        error = %e,
                        "Startup recovery: failed to fetch promise, skipping"
                    );
                    continue;
                }
                Err(_) => {
                    warn!(
                        promise_id = %promise.id,
                        "Startup recovery: get_promise timed out, skipping"
                    );
                    continue;
                }
            }

            // For built-in handlers, check the feature flag BEFORE claiming the
            // promise. Without this, prepare_agent_with_promise would refresh
            // claimed_at even when the handler is disabled, causing the promise
            // to appear stale again every 5 minutes and cycle until the 24-hour
            // TTL — wasting KV writes and cluttering the log with spurious
            // "resuming stale promise" entries.
            //
            // Automation runs do not have per-handler feature flags at the runner
            // level, so only built-in handler runs (automation_id is empty) need
            // this pre-check.
            if promise.automation_id.is_empty() {
                let flag_enabled = match promise.nats_subject.as_str() {
                    s if s.starts_with("github.pull_request") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::PrReviewEnabled)
                            .await
                    }
                    s if s.starts_with("github.check_run") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::CiHandlerEnabled)
                            .await
                    }
                    s if s.starts_with("github.push") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::PushHandlerEnabled)
                            .await
                    }
                    s if s.starts_with("github.issue_comment") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::CommentHandlerEnabled)
                            .await
                    }
                    s if s.starts_with("linear.") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::IssueTriageEnabled)
                            .await
                    }
                    s if s.starts_with("datadog.") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::AlertHandlerEnabled)
                            .await
                    }
                    s if s.starts_with("incidentio.") => {
                        agent
                            .is_flag_enabled(&crate::flags::AgentFlag::IncidentioHandlerEnabled)
                            .await
                    }
                    // Unknown subjects pass through to the warning branch in the
                    // dispatch match below.
                    _ => true,
                };
                if !flag_enabled {
                    info!(
                        promise_id = %promise.id,
                        subject = %promise.nats_subject,
                        "Startup recovery: handler disabled by feature flag — skipping without claiming"
                    );
                    continue;
                }
            }

            // CAS-claiming is delegated to prepare_agent_with_promise below.
            // This avoids a double-write and ensures both the startup recovery
            // path and the NATS redelivery path use the same ownership mechanism,
            // preventing concurrent execution of the same promise.
            info!(
                promise_id = %promise.id,
                subject = %promise.nats_subject,
                iteration = promise.iteration,
                "Startup recovery: resuming stale promise"
            );

            let payload: bytes::Bytes = serde_json::to_vec(&promise.trigger)
                .unwrap_or_default()
                .into();

            let Some(agent) = prepare_agent_with_promise(
                &agent,
                &promise_store,
                &tenant_id,
                &promise.id,
                &promise.automation_id,
                &promise.nats_subject,
                &promise.trigger,
            )
            .await else {
                info!(
                    promise_id = %promise.id,
                    "Startup recovery: CAS claim lost — another worker took this promise"
                );
                continue;
            };

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
                    // Mark PermanentFailed — the automation is gone permanently;
                    // retrying would always reach this arm and cycle until the TTL.
                    // (Failed would be semantically wrong: it implies a transient
                    // error that NATS redelivery can fix, which is not the case here.)
                    warn!(
                        promise_id = %promise.id,
                        automation_id = %promise.automation_id,
                        "Startup recovery: automation no longer exists — marking promise PermanentFailed"
                    );
                    match tokio::time::timeout(
                        crate::agent_loop::NATS_KV_TIMEOUT,
                        promise_store.get_promise(&tenant_id, &promise.id),
                    )
                    .await
                    {
                        Ok(Ok(Some((mut current, rev)))) => {
                            current.status = PromiseStatus::PermanentFailed;
                            match tokio::time::timeout(
                                crate::agent_loop::NATS_KV_TIMEOUT,
                                promise_store.update_promise(
                                    &tenant_id,
                                    &promise.id,
                                    &current,
                                    rev,
                                ),
                            )
                            .await
                            {
                                Ok(Ok(_)) => {}
                                Ok(Err(e)) => warn!(
                                    promise_id = %promise.id,
                                    error = %e,
                                    "Startup recovery: failed to mark orphaned promise PermanentFailed"
                                ),
                                Err(_) => warn!(
                                    promise_id = %promise.id,
                                    "Startup recovery: update_promise timed out marking orphaned promise PermanentFailed"
                                ),
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(e)) => warn!(
                            promise_id = %promise.id,
                            error = %e,
                            "Startup recovery: get_promise failed for orphaned promise"
                        ),
                        Err(_) => warn!(
                            promise_id = %promise.id,
                            "Startup recovery: get_promise timed out for orphaned promise"
                        ),
                    }
                }
            } else {
                // Built-in handler dispatch based on NATS subject.
                // Feature flags were already checked before the CAS-claim above,
                // so no per-arm flag check is needed here.
                match promise.nats_subject.as_str() {
                    s if s.starts_with("github.pull_request") => {
                        let pv: serde_json::Value =
                            serde_json::from_slice(&payload).unwrap_or_default();
                        let is_merged = pv["action"].as_str() == Some("closed")
                            && pv["pull_request"]["merged"].as_bool() == Some(true);
                        if is_merged {
                            handlers::pr_merged::handle(&agent, &payload).await;
                        } else {
                            handlers::pr_review::handle(&agent, &payload).await;
                        }
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
                    s if s.starts_with("datadog.") => {
                        handlers::alert_triggered::handle(&agent, &promise.nats_subject, &payload)
                            .await;
                    }
                    s if s.starts_with("incidentio.") => {
                        handlers::incident_declared::handle(
                            &agent,
                            &promise.nats_subject,
                            &payload,
                        )
                        .await;
                    }
                    other => {
                        // The subject is no longer handled by any built-in dispatcher
                        // (renamed, removed, or a deployment artifact). Mark the promise
                        // PermanentFailed so neither startup recovery nor NATS redelivery
                        // picks it up again — retrying with an unknown subject will always
                        // reach this arm and cycle until the 24 h TTL.
                        warn!(
                            subject = %other,
                            promise_id = %promise.id,
                            "Startup recovery: unknown subject — marking promise PermanentFailed"
                        );
                        match tokio::time::timeout(
                            crate::agent_loop::NATS_KV_TIMEOUT,
                            promise_store.get_promise(&tenant_id, &promise.id),
                        )
                        .await
                        {
                            Ok(Ok(Some((mut current, rev)))) => {
                                current.status = PromiseStatus::PermanentFailed;
                                match tokio::time::timeout(
                                    crate::agent_loop::NATS_KV_TIMEOUT,
                                    promise_store.update_promise(&tenant_id, &promise.id, &current, rev),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => {}
                                    Ok(Err(e)) => warn!(
                                        promise_id = %promise.id,
                                        error = %e,
                                        "Startup recovery: failed to mark unknown-subject promise as PermanentFailed"
                                    ),
                                    Err(_) => warn!(
                                        promise_id = %promise.id,
                                        "Startup recovery: update_promise timed out marking unknown-subject promise as PermanentFailed"
                                    ),
                                }
                            }
                            Ok(Ok(None)) => {}
                            Ok(Err(e)) => warn!(
                                promise_id = %promise.id,
                                error = %e,
                                "Startup recovery: get_promise failed for unknown-subject promise"
                            ),
                            Err(_) => warn!(
                                promise_id = %promise.id,
                                "Startup recovery: get_promise timed out for unknown-subject promise"
                            ),
                        }
                    }
                }
            }
        }
    }))
}

/// Spawn a background task that sends [`AckKind::Progress`] every 15 seconds
/// to prevent JetStream from redelivering the message while the agent is active.
///
/// The returned handle **must** be aborted once processing finishes so the task
/// stops sending progress acks after the message has been explicitly acked.
fn spawn_heartbeat(msg: async_nats::jetstream::Message) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
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
pub(crate) async fn dispatch_automations<R: RunRepository>(
    agent: &Arc<AgentLoop>,
    run_store: &Arc<R>,
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
                let Some(agent) = prepare_agent_with_promise(
                    &agent,
                    &promise_store,
                    &tenant_id,
                    &promise_id,
                    &auto.id,
                    &subject,
                    &trigger,
                )
                .await else {
                    info!(
                        automation = %auto.name,
                        promise_id = %promise_id,
                        "CAS claim lost — another worker is running this automation, skipping"
                    );
                    return;
                };

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
                // 20 redeliveries before NATS gives up. With durable promises,
                // every redelivery is safe — the agent resumes from the KV
                // checkpoint rather than starting from scratch. 20 absorbs any
                // realistic crash/restart sequence while still bounding retries
                // for genuinely broken events (bad payload, persistent bug)
                // so they don't consume Anthropic credits indefinitely.
                max_deliver: 20,
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

    /// Start a throwaway NATS container and open both `AutomationStore` and
    /// `RunStore` from it.  Returns the two stores and an opaque `impl Drop`
    /// guard — keep the guard alive for the duration of the test.
    async fn make_all_stores() -> (
        std::sync::Arc<trogon_automations::AutomationStore>,
        std::sync::Arc<trogon_automations::RunStore>,
        impl Drop,
    ) {
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
        let auto_store = std::sync::Arc::new(
            trogon_automations::AutomationStore::open(&js)
                .await
                .expect("AutomationStore"),
        );
        let run_store = std::sync::Arc::new(
            trogon_automations::RunStore::open(&js)
                .await
                .expect("RunStore"),
        );
        (auto_store, run_store, container)
    }

    fn make_agent_off() -> Arc<crate::agent_loop::AgentLoop> {
        use crate::agent_loop::ReqwestAnthropicClient;
        use crate::tools::{DefaultToolDispatcher, ToolContext};
        struct AlwaysOffFlagClient;
        impl crate::flag_client::FeatureFlagClient for AlwaysOffFlagClient {
            fn is_enabled<'a>(
                &'a self,
                _tenant_id: &'a str,
                _flag: &'a dyn trogon_splitio::flags::FeatureFlag,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
                Box::pin(async { false })
            }
        }
        let http_client = reqwest::Client::new();
        let tool_ctx = Arc::new(ToolContext::for_test(
            "http://127.0.0.1:1",
            "tok_test",
            "",
            "",
        ));
        Arc::new(crate::agent_loop::AgentLoop {
            anthropic_client: Arc::new(ReqwestAnthropicClient::new(
                http_client,
                "http://127.0.0.1:1".to_string(),
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
            flag_client: Arc::new(AlwaysOffFlagClient),
            tenant_id: "acme".to_string(),
            promise_store: None,
            promise_id: None,
        })
    }

    /// Build a stale `Running` promise whose `claimed_at` is far enough in the
    /// past to exceed the 10-minute staleness threshold inside
    /// `recover_stale_promises`.
    fn make_stale_promise(id: &str) -> crate::promise_store::AgentPromise {
        crate::promise_store::AgentPromise {
            id: id.to_string(),
            tenant_id: "acme".to_string(),
            automation_id: String::new(),
            status: crate::promise_store::PromiseStatus::Running,
            messages: vec![],
            iteration: 3,
            worker_id: "old-worker".to_string(),
            // 20 minutes in the past — well past STALE_AFTER_SECS (10 min).
            claimed_at: trogon_automations::now_unix().saturating_sub(20 * 60),
            trigger: serde_json::json!({}),
            nats_subject: "github.pull_request".to_string(),
            system_prompt: None,
        }
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

    // ── prepare_agent_with_promise ────────────────────────────────────────────

    fn make_empty_store() -> Arc<dyn crate::promise_store::PromiseRepository> {
        Arc::new(crate::promise_store::mock::MockPromiseStore::new())
    }

    fn make_store_with(
        promise: crate::promise_store::AgentPromise,
    ) -> Arc<crate::promise_store::mock::MockPromiseStore> {
        let store = Arc::new(crate::promise_store::mock::MockPromiseStore::new());
        store.insert_promise(promise);
        store
    }

    fn sample_promise(id: &str, status: crate::promise_store::PromiseStatus) -> crate::promise_store::AgentPromise {
        crate::promise_store::AgentPromise {
            id: id.to_string(),
            tenant_id: "acme".to_string(),
            automation_id: String::new(),
            status,
            messages: vec![],
            iteration: 0,
            worker_id: "old-worker".to_string(),
            claimed_at: 0,
            trigger: serde_json::Value::Null,
            nats_subject: "github.pull_request".to_string(),
            system_prompt: None,
        }
    }

    /// When no promise exists for a new trigger, `prepare_agent_with_promise`
    /// creates one in KV (`Running`) and returns an `AgentLoop` wired with the
    /// store and promise ID.
    #[tokio::test]
    async fn prepare_agent_creates_promise_when_none_exists() {
        let store = make_empty_store();
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &store,
            "acme",
            "p-new",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_some(), "must return an agent for a new promise");
        // The returned agent must be wired with the promise.
        let returned = result.unwrap();
        assert_eq!(returned.promise_id.as_deref(), Some("p-new"));

        // A Running promise must have been created in KV.
        let (p, _) = store.get_promise("acme", "p-new").await.unwrap().unwrap();
        assert_eq!(p.status, crate::promise_store::PromiseStatus::Running);
        assert_eq!(p.id, "p-new");
    }

    /// When an existing `Running` promise is found, the runner CAS-claims it
    /// (updating `worker_id` and `claimed_at`) and returns an agent wired with
    /// the store so recovery can resume from the checkpoint.
    #[tokio::test]
    async fn prepare_agent_claims_running_promise() {
        use crate::promise_store::PromiseRepository;

        let store = make_store_with(sample_promise("p1", crate::promise_store::PromiseStatus::Running));
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>),
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_some(), "must return agent for a Running promise");

        // worker_id must have been updated (CAS claim happened).
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_ne!(p.worker_id, "old-worker", "worker_id must be updated by the CAS claim");
    }

    /// A `Resolved` promise means the run already completed. The function must
    /// return `None` so the caller acks the NATS message without re-running —
    /// avoiding duplicate LLM calls and tool side-effects.
    #[tokio::test]
    async fn prepare_agent_returns_none_for_resolved_promise() {
        let store = make_store_with(sample_promise("p1", crate::promise_store::PromiseStatus::Resolved));
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>),
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_none(), "must return None for Resolved promise — caller acks without re-running");
    }

    /// When the CAS claim of a `Running` promise fails (another worker claimed
    /// it first), `prepare_agent_with_promise` returns `None` so this worker
    /// skips the run and avoids concurrent execution.
    #[tokio::test]
    async fn prepare_agent_returns_none_on_cas_claim_conflict() {
        let store = Arc::new(crate::promise_store::mock::CasConflictOnceStore::new());
        store.inner.insert_promise(sample_promise("p1", crate::promise_store::PromiseStatus::Running));
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>),
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(
            result.is_none(),
            "must return None when CAS claim fails — another worker already owns the run"
        );
    }

    /// A `PermanentFailed` promise must be treated identically to `Resolved`:
    /// return `None` so the caller acks without re-running. Retrying a
    /// deterministic failure (e.g. max_tokens) would always produce the same
    /// outcome and waste Anthropic credits.
    #[tokio::test]
    async fn prepare_agent_returns_none_for_permanent_failed_promise() {
        let store = make_store_with(sample_promise(
            "p1",
            crate::promise_store::PromiseStatus::PermanentFailed,
        ));
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>),
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(
            result.is_none(),
            "must return None for PermanentFailed promise — same as Resolved, never re-run"
        );
    }

    /// A `Failed` promise (transient error) must be re-claimed and resumed,
    /// not skipped. `Failed` means "retry makes sense" — NATS redelivered the
    /// message after a transient failure and this worker should take over.
    #[tokio::test]
    async fn prepare_agent_claims_failed_promise_for_retry() {
        use crate::promise_store::PromiseRepository;

        let store = make_store_with(sample_promise(
            "p1",
            crate::promise_store::PromiseStatus::Failed,
        ));
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &(Arc::clone(&store) as Arc<dyn crate::promise_store::PromiseRepository>),
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_some(), "must return agent for a Failed promise — transient error, retry is valid");

        // Promise must be re-claimed (worker_id updated, status reset to Running).
        let (p, _) = store.get_promise("acme", "p1").await.unwrap().unwrap();
        assert_eq!(
            p.status,
            crate::promise_store::PromiseStatus::Running,
            "Failed promise must be reset to Running on re-claim"
        );
        assert_ne!(p.worker_id, "old-worker", "worker_id must be updated by the re-claim");
    }

    // ── prepare_agent timeout / error paths ───────────────────────────────────

    /// When the initial `get_promise` KV call times out, the function must
    /// log a warning and return `Some(original_agent)` — the original agent
    /// without `promise_store` or `promise_id` wired so the run proceeds
    /// without durability rather than blocking indefinitely.
    #[tokio::test(start_paused = true)]
    async fn prepare_agent_get_promise_timeout_returns_agent_without_durability() {
        use crate::promise_store::mock::HangingGetPromiseStore;
        use crate::promise_store::PromiseRepository;
        use std::time::Duration;

        let store = Arc::new(HangingGetPromiseStore::new());
        let store_arc = Arc::clone(&store) as Arc<dyn PromiseRepository>;
        let agent = make_agent("http://127.0.0.1:1");
        let trigger = serde_json::json!({});

        let (result, _) = tokio::join!(
            super::prepare_agent_with_promise(
                &agent,
                &store_arc,
                "acme",
                "p1",
                "",
                "github.pull_request",
                &trigger,
            ),
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1)
            ),
        );

        let returned = result.expect("must return Some(agent) when get_promise times out");
        assert!(
            returned.promise_id.is_none(),
            "returned agent must have no promise_id — run proceeds without durability on timeout"
        );
        assert!(
            returned.promise_store.is_none(),
            "returned agent must have no promise_store — run proceeds without durability on timeout"
        );
    }

    /// When the CAS claim `update_promise` call times out, the function must
    /// return `None` — this worker cannot safely own the promise, so the run
    /// is skipped to avoid concurrent execution with another worker.
    #[tokio::test(start_paused = true)]
    async fn prepare_agent_cas_claim_timeout_returns_none() {
        use crate::promise_store::mock::HangingUpdateStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use std::time::Duration;

        let store = Arc::new(HangingUpdateStore::new());
        store
            .inner
            .insert_promise(sample_promise("p1", PromiseStatus::Running));
        let store_arc = Arc::clone(&store) as Arc<dyn PromiseRepository>;
        let agent = make_agent("http://127.0.0.1:1");
        let trigger = serde_json::json!({});

        let (result, _) = tokio::join!(
            super::prepare_agent_with_promise(
                &agent,
                &store_arc,
                "acme",
                "p1",
                "",
                "github.pull_request",
                &trigger,
            ),
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1)
            ),
        );

        assert!(
            result.is_none(),
            "must return None when the CAS claim update_promise times out"
        );
    }

    /// When `get_promise` returns an immediate `Err`, the function must log a
    /// warning and return `Some(original_agent)` without durability fields —
    /// the run proceeds as non-durable rather than aborting.
    ///
    /// Distinct from the timeout path (which hits the outer `Err(_)` arm of
    /// `tokio::time::timeout`); this hits the inner `Err(e)` arm of the
    /// `match get_result` block.
    #[tokio::test]
    async fn prepare_agent_get_promise_error_returns_agent_without_durability() {
        use crate::promise_store::mock::ErrorGetPromiseStore;
        use crate::promise_store::PromiseRepository;

        let store = Arc::new(ErrorGetPromiseStore::new());
        let store_arc = Arc::clone(&store) as Arc<dyn PromiseRepository>;
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &store_arc,
            "acme",
            "p1",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        let returned = result.expect("must return Some(agent) when get_promise errors");
        assert!(
            returned.promise_id.is_none(),
            "returned agent must have no promise_id — run proceeds without durability on error"
        );
        assert!(
            returned.promise_store.is_none(),
            "returned agent must have no promise_store — run proceeds without durability on error"
        );
    }

    /// When `put_promise` returns an immediate `Err` (new promise path — no
    /// existing KV entry), the function must log a warning and still return
    /// `Some(agent)` with `promise_id` and `promise_store` wired.
    ///
    /// The KV write failing means this run has no durable checkpoint, but it
    /// should still proceed rather than abort — the `Ok(Err(e))` arm of the
    /// inner `put_promise` match falls through to the wiring block.
    #[tokio::test]
    async fn prepare_agent_put_promise_error_returns_agent_with_promise_id_wired() {
        use crate::promise_store::mock::ErrorPutPromiseStore;
        use crate::promise_store::PromiseRepository;

        let store = Arc::new(ErrorPutPromiseStore::new());
        let store_arc = Arc::clone(&store) as Arc<dyn PromiseRepository>;
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &store_arc,
            "acme",
            "p-new",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        let returned = result.expect("must return Some(agent) even when put_promise errors");
        assert_eq!(
            returned.promise_id.as_deref(),
            Some("p-new"),
            "promise_id must be wired despite put_promise error"
        );
        assert!(
            returned.promise_store.is_some(),
            "promise_store must be wired despite put_promise error"
        );
    }

    /// When `put_promise` hangs and the KV timeout fires (new promise path),
    /// the function must log a warning and still return `Some(agent)` with
    /// `promise_id` and `promise_store` wired.
    ///
    /// The `Err(_)` timeout arm of `tokio::time::timeout(put_promise)` falls
    /// through to the wiring block — the run proceeds without a durable
    /// checkpoint but is not aborted.
    #[tokio::test(start_paused = true)]
    async fn prepare_agent_put_promise_timeout_returns_agent_with_promise_id_wired() {
        use crate::promise_store::mock::HangingPutPromiseStore;
        use crate::promise_store::PromiseRepository;
        use std::time::Duration;

        let store = Arc::new(HangingPutPromiseStore::new());
        let store_arc = Arc::clone(&store) as Arc<dyn PromiseRepository>;
        let agent = make_agent("http://127.0.0.1:1");
        let trigger = serde_json::json!({});

        let (result, _) = tokio::join!(
            super::prepare_agent_with_promise(
                &agent,
                &store_arc,
                "acme",
                "p-new",
                "",
                "github.pull_request",
                &trigger,
            ),
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1)
            ),
        );

        let returned = result.expect("must return Some(agent) when put_promise times out");
        assert_eq!(
            returned.promise_id.as_deref(),
            Some("p-new"),
            "promise_id must be wired despite put_promise timeout"
        );
        assert!(
            returned.promise_store.is_some(),
            "promise_store must be wired despite put_promise timeout"
        );
    }

    // ── recover_stale_promises ────────────────────────────────────────────────

    /// When `list_running` times out, `recover_stale_promises` must return
    /// `None` (no recovery task spawned) immediately after the timeout.
    ///
    /// Note: `start_paused = true` would freeze the clock before testcontainers
    /// can start Docker, so we pause the clock manually after store setup.
    #[tokio::test]
    async fn recover_list_running_timeout_returns_none() {
        use crate::promise_store::mock::HangingListRunningStore;
        use crate::promise_store::PromiseRepository;
        use std::time::Duration;

        // Set up the stores with a real clock so Docker/NATS can start.
        let (auto_store, run_store, _container) = make_all_stores().await;

        // Now pause the clock — `HangingListRunningStore::list_running` returns
        // `pending()`, so `recover_stale_promises` blocks on the timeout future.
        tokio::time::pause();

        let promise_store: Arc<dyn PromiseRepository> =
            Arc::new(HangingListRunningStore::new());
        let agent = make_agent("http://127.0.0.1:1");

        let (handle, _) = tokio::join!(
            super::recover_stale_promises(
                &agent,
                &promise_store,
                &auto_store,
                &run_store,
                "acme",
            ),
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1)
            ),
        );

        assert!(
            handle.is_none(),
            "no recovery task must be spawned when list_running times out"
        );
    }

    /// When `list_running` returns an immediate error, `recover_stale_promises`
    /// must return `None` without spawning a recovery task.
    #[tokio::test]
    async fn recover_list_running_error_returns_none() {
        use crate::promise_store::mock::ErrorListRunningStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let promise_store: Arc<dyn PromiseRepository> =
            Arc::new(ErrorListRunningStore::new());
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await;

        assert!(
            handle.is_none(),
            "no recovery task must be spawned when list_running errors"
        );
    }

    /// When `list_running` returns an empty list (no Running promises at all),
    /// `recover_stale_promises` must return `None`.
    #[tokio::test]
    async fn recover_empty_running_list_returns_none() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let promise_store: Arc<dyn PromiseRepository> =
            Arc::new(MockPromiseStore::new());
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await;

        assert!(
            handle.is_none(),
            "no recovery task must be spawned when no Running promises exist"
        );
    }

    /// When Running promises exist but none are stale (all have a recent
    /// `claimed_at`), the stale filter produces an empty list and
    /// `recover_stale_promises` must return `None`.
    #[tokio::test]
    async fn recover_no_stale_promises_returns_none() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus};

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(MockPromiseStore::new());

        // claimed_at = now → age = 0s, well under STALE_AFTER_SECS (600s).
        let fresh = AgentPromise {
            id: "p-fresh".to_string(),
            tenant_id: "acme".to_string(),
            automation_id: String::new(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "w".to_string(),
            claimed_at: trogon_automations::now_unix(),
            trigger: serde_json::Value::Null,
            nats_subject: "github.pull_request".to_string(),
            system_prompt: None,
        };
        store.insert_promise(fresh);

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await;

        assert!(
            handle.is_none(),
            "no recovery task must be spawned when all Running promises are fresh"
        );
    }

    /// When the re-fetch of a stale promise returns `Ok(None)` (the promise
    /// was deleted from KV between the scan and the recovery loop), the loop
    /// must skip that promise without claiming or modifying any state.
    #[tokio::test]
    async fn recover_refetch_vanished_skips_promise() {
        use crate::promise_store::mock::VanishedOnRefetchStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(VanishedOnRefetchStore::new());
        // Populate inner so list_running returns the stale promise.
        store.inner.insert_promise(make_stale_promise("p-vanished"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        // The inner store must still hold the original Running promise with
        // unchanged worker_id — the vanished re-fetch caused a skip.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-vanished")
            .await
            .unwrap()
            .expect("promise must still be in inner store");
        assert_eq!(
            p.worker_id, "old-worker",
            "worker_id must not change — promise was skipped after vanishing on re-fetch"
        );
    }

    /// When the re-fetch returns the promise with a non-Running status
    /// (another worker completed it between the scan and the re-fetch),
    /// the loop must skip it without claiming.
    #[tokio::test]
    async fn recover_refetch_resolved_skips_promise() {
        use crate::promise_store::mock::ResolvedOnRefetchStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(ResolvedOnRefetchStore::new());
        store.inner.insert_promise(make_stale_promise("p-resolved"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        // Promise must remain with unchanged worker_id — re-fetch returned
        // Resolved so the loop skipped without claiming.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-resolved")
            .await
            .unwrap()
            .expect("promise must still be in inner store");
        assert_eq!(
            p.worker_id, "old-worker",
            "worker_id must not change — promise was already Resolved on re-fetch"
        );
    }

    /// When `get_promise` returns an error during the re-fetch inside the
    /// recovery loop, the loop must skip that promise and continue.
    #[tokio::test]
    async fn recover_refetch_error_skips_promise() {
        use crate::promise_store::mock::ErrorGetPromiseStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(ErrorGetPromiseStore::new());
        // Populate inner so list_running returns the stale promise (list_running
        // delegates to inner; get_promise errors — that's the re-fetch failure).
        store.inner.insert_promise(make_stale_promise("p-err"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        // Must not panic — the error is logged and the loop continues.
        handle.await.expect("recovery task must not panic");
    }

    /// When the feature flag for a built-in handler is disabled, the recovery
    /// loop must skip the promise without calling `prepare_agent_with_promise`
    /// (no CAS claim, no worker_id update).
    #[tokio::test]
    async fn recover_flag_disabled_skips_without_claiming() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(MockPromiseStore::new());
        store.insert_promise(make_stale_promise("p-flag-off"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        // AlwaysOffFlagClient — all feature flags return false.
        let agent = make_agent_off();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        // worker_id must be unchanged — flag was off so we never claimed.
        let (p, _) = store
            .get_promise("acme", "p-flag-off")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.worker_id, "old-worker",
            "worker_id must not change when feature flag is disabled"
        );
    }

    /// When a stale promise has an unknown `nats_subject` (built-in handler
    /// dispatch falls through to the `other =>` arm), the recovery must mark
    /// it `PermanentFailed` so neither startup recovery nor NATS redelivery
    /// cycles on it again until the 24 h TTL expires.
    #[tokio::test]
    async fn recover_unknown_subject_marks_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(MockPromiseStore::new());

        let mut p = make_stale_promise("p-unknown");
        p.nats_subject = "completely.unknown.subject".to_string();
        store.insert_promise(p);

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .get_promise("acme", "p-unknown")
            .await
            .unwrap()
            .expect("promise must still exist after recovery");
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "unknown subject must mark promise PermanentFailed"
        );
    }

    /// When `prepare_agent_with_promise` fails to CAS-claim the promise
    /// (another worker took it between the re-fetch and the claim), the
    /// recovery loop must skip without re-running the agent.
    #[tokio::test]
    async fn recover_cas_claim_lost_skips_promise() {
        use crate::promise_store::mock::CasConflictOnceStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(CasConflictOnceStore::new());
        store.inner.insert_promise(make_stale_promise("p-cas"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        // Promise status must still be Running — no handler was called because
        // the CAS claim returned None.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-cas")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "promise must remain Running when CAS claim is lost"
        );
    }

    /// When the re-fetch inside the recovery task times out (`get_promise`
    /// hangs for longer than `NATS_KV_TIMEOUT`), the loop must skip that
    /// promise and continue without panicking.
    ///
    /// `HangingGetPromiseStore` makes every `get_promise` return
    /// `std::future::pending()`.  The clock is paused manually after container
    /// setup to avoid breaking testcontainers, then advanced past the timeout
    /// while awaiting the spawned handle.
    #[tokio::test]
    async fn recover_refetch_timeout_skips_promise() {
        use crate::promise_store::mock::HangingGetPromiseStore;
        use crate::promise_store::PromiseRepository;
        use std::time::Duration;

        // Start the NATS container with the real clock so Docker can reach the
        // network properly.
        let (auto_store, run_store, _container) = make_all_stores().await;

        // Now freeze the clock — all subsequent `tokio::time::timeout` calls
        // will block until we call `tokio::time::advance`.
        tokio::time::pause();

        let store = Arc::new(HangingGetPromiseStore::new());
        // Populate inner so `list_running` returns a stale promise.
        // `get_promise` is overridden to hang forever.
        store.inner.insert_promise(make_stale_promise("p-hang"));

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        // Advance the mock clock past NATS_KV_TIMEOUT to fire the re-fetch
        // timeout inside the spawned task.
        let (join_result, _) = tokio::join!(
            handle,
            tokio::time::advance(crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1)),
        );
        join_result.expect("recovery task must not panic");

        // worker_id must be unchanged — the re-fetch timed out so the promise
        // was skipped without being claimed.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-hang")
            .await
            .unwrap()
            .expect("promise must still exist in inner store");
        assert_eq!(
            p.worker_id, "old-worker",
            "worker_id must not change — re-fetch timed out and promise was skipped"
        );
    }

    /// `recover_stale_promises` must process at most `MAX_RECOVERY_AT_STARTUP`
    /// (= 10) stale promises.  When more than 10 are stale, the remainder are
    /// deferred to the next restart.
    ///
    /// Scenario: 11 stale promises with an unknown `nats_subject`.  The
    /// unknown-subject arm marks each processed promise `PermanentFailed`
    /// without making any HTTP calls, so the test completes instantly.
    /// After recovery exactly 10 must be `PermanentFailed` and 1 must still
    /// be `Running`.
    #[tokio::test]
    async fn recover_stale_cap_processes_at_most_10() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(MockPromiseStore::new());

        // Insert 11 stale promises — one more than the cap.
        // All have an unknown subject so each is handled synchronously
        // (no HTTP) by the `other =>` arm that marks PermanentFailed.
        for i in 0..11u32 {
            let mut p = make_stale_promise(&format!("p-cap-{i}"));
            p.nats_subject = "completely.unknown.subject".to_string();
            store.insert_promise(p);
        }

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let all = store.snapshot_promises();
        let permanent_failed = all
            .values()
            .filter(|(p, _)| p.status == PromiseStatus::PermanentFailed)
            .count();
        let still_running = all
            .values()
            .filter(|(p, _)| p.status == PromiseStatus::Running)
            .count();

        assert_eq!(
            permanent_failed, 10,
            "exactly 10 promises must be processed (cap = MAX_RECOVERY_AT_STARTUP); got {permanent_failed}"
        );
        assert_eq!(
            still_running, 1,
            "exactly 1 promise must remain Running — deferred to next restart; got {still_running}"
        );
    }

    /// When a stale promise has a non-empty `automation_id` but the automation
    /// no longer exists in the `AutomationStore`, the recovery loop must mark
    /// the promise `PermanentFailed`.
    ///
    /// Rationale: the automation was deleted after the crash.  Retrying would
    /// always reach the "automation not found" arm and cycle until the 24 h
    /// TTL — marking `PermanentFailed` stops that cycle immediately.
    #[tokio::test]
    async fn recover_automation_deleted_marks_permanent_failed() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        // Real AutomationStore — intentionally empty so every lookup fails.
        let (auto_store, run_store, _container) = make_all_stores().await;
        let store = Arc::new(MockPromiseStore::new());

        // Stale promise that belongs to an automation that no longer exists.
        let mut p = make_stale_promise("p-del-auto");
        p.automation_id = "deleted-auto-id".to_string();
        p.nats_subject = "github.push".to_string();
        store.insert_promise(p);

        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .get_promise("acme", "p-del-auto")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "promise for a deleted automation must be marked PermanentFailed so recovery stops cycling"
        );
    }

    // ── dispatch_automations — additional edge cases ───────────────────────────

    /// When the NATS payload is not valid JSON, `serde_json::from_slice`
    /// fails and `unwrap_or_default()` falls back to `Value::Null`.
    /// The automation must still run and the created promise must carry
    /// `trigger = Null`.
    #[tokio::test]
    async fn dispatch_automations_invalid_json_payload_stores_null_trigger() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
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
        let promise_store = Arc::new(MockPromiseStore::new());
        let promise_store_arc = Arc::clone(&promise_store) as Arc<dyn PromiseRepository>;

        // Deliberately non-JSON payload.
        let payload = bytes::Bytes::from_static(b"not-valid-json");

        dispatch_automations(
            &agent,
            &Arc::new(rs),
            &promise_store_arc,
            "github.42",
            vec![make_automation("auto-1")],
            "github.push",
            &payload,
        )
        .await;

        // The promise must have been created with trigger = Null (unwrap_or_default).
        let snapshot = promise_store.snapshot_promises();
        assert_eq!(snapshot.len(), 1, "exactly one promise must be created");
        let (p, _) = snapshot.values().next().unwrap();
        assert_eq!(
            p.trigger,
            serde_json::Value::Null,
            "invalid JSON payload must produce trigger = Null via unwrap_or_default"
        );
    }

    /// Each automation spawned by `dispatch_automations` must receive a
    /// distinct promise ID so concurrent automations on the same NATS
    /// message checkpoint independently.
    ///
    /// The promise ID format is `{prefix}.{automation_id}`.
    #[tokio::test]
    async fn dispatch_automations_each_automation_gets_unique_promise_id() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
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
        let promise_store = Arc::new(MockPromiseStore::new());
        let promise_store_arc = Arc::clone(&promise_store) as Arc<dyn PromiseRepository>;
        let payload = bytes::Bytes::from_static(b"{}");

        dispatch_automations(
            &agent,
            &Arc::new(rs),
            &promise_store_arc,
            "github.42",
            vec![make_automation("auto-A"), make_automation("auto-B")],
            "github.push",
            &payload,
        )
        .await;

        let snapshot = promise_store.snapshot_promises();
        assert_eq!(snapshot.len(), 2, "two automations must produce two distinct promises");

        let ids: std::collections::HashSet<String> =
            snapshot.values().map(|(p, _)| p.id.clone()).collect();
        assert_eq!(ids.len(), 2, "promise IDs must be distinct");

        // Each promise ID must encode the automation ID.
        for (_, (p, _)) in &snapshot {
            assert!(
                p.id.contains("auto-A") || p.id.contains("auto-B"),
                "promise ID must embed the automation ID; got: {}",
                p.id
            );
        }
    }

    /// When a spawned automation task panics (e.g. due to an LLM client
    /// panic), `dispatch_automations` catches the `JoinError`, logs it, and
    /// returns normally — it must not propagate the panic to the caller.
    #[tokio::test]
    async fn dispatch_automations_panicking_task_is_caught() {
        use crate::agent_loop::{AgentLoop, AnthropicClient};
        use crate::flag_client::AlwaysOnFlagClient;
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use crate::tools::{DefaultToolDispatcher, ToolContext};

        struct PanicClient;
        impl AnthropicClient for PanicClient {
            fn complete<'a>(
                &'a self,
                _body: serde_json::Value,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<serde_json::Value, reqwest::Error>,
                        > + Send
                        + 'a,
                >,
            > {
                panic!("intentional panic from PanicClient");
            }
        }

        let tool_ctx = Arc::new(ToolContext::for_test("http://127.0.0.1:1", "", "", ""));
        let agent = Arc::new(AgentLoop {
            anthropic_client: Arc::new(PanicClient),
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
        });

        let (rs, _container) = make_run_store().await;
        let promise_store: Arc<dyn PromiseRepository> =
            Arc::new(MockPromiseStore::new());
        let payload = bytes::Bytes::from_static(b"{}");

        // Must return without panicking even though the task panics.
        dispatch_automations(
            &agent,
            &Arc::new(rs),
            &promise_store,
            "github.42",
            vec![make_automation("panic-auto")],
            "github.push",
            &payload,
        )
        .await;
    }

    /// When `list_tools` returns a 200 response whose body parses as valid JSON
    /// but whose `result` field cannot be deserialized as `ListToolsResult`,
    /// `init_mcp_servers` must skip the server and return empty vectors.
    ///
    /// This is distinct from the 500-error case: here the HTTP call
    /// "succeeds" but the payload is semantically wrong.
    #[tokio::test]
    async fn init_mcp_servers_list_tools_deserialize_error_skips_server() {
        let server = httpmock::MockServer::start_async().await;

        // initialize succeeds.
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .body_contains("\"initialize\"");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "serverInfo": {}
                    }
                }));
        });

        // list_tools returns 200 but with a `result` that is a plain string,
        // not the expected `{ tools: [...] }` object → deserialization error.
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .body_contains("tools/list");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0", "id": 2,
                    "result": "this is not a ListToolsResult"
                }));
        });

        let cfg = vec![crate::config::McpServerConfig {
            name: "bad-result-server".to_string(),
            url: format!("{}/mcp", server.base_url()),
        }];
        let http_client = reqwest::Client::new();
        let (tools, dispatch) = super::init_mcp_servers(&http_client, &cfg).await;

        assert!(
            tools.is_empty(),
            "no tools expected when list_tools result cannot be deserialized"
        );
        assert!(
            dispatch.is_empty(),
            "no dispatch entries expected when list_tools result cannot be deserialized"
        );
    }

    // ── recover_stale_promises — error paths inside the recovery loop ─────────

    /// In-memory automation store whose `list()` always returns an error.
    ///
    /// Used to verify that a transient `AutomationStore` failure during startup
    /// recovery causes the affected promise to be skipped (kept in `Running`)
    /// rather than incorrectly marked `PermanentFailed`.
    #[derive(Clone)]
    struct ErrorListAutomationStore;

    impl trogon_automations::AutomationRepository for ErrorListAutomationStore {
        fn put<'a>(
            &'a self,
            _automation: &'a trogon_automations::Automation,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(()) })
        }

        fn get<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<trogon_automations::Automation>, trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }

        fn delete<'a>(
            &'a self,
            _tenant_id: &'a str,
            _id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(()) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<trogon_automations::Automation>, trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(trogon_automations::store::StoreError(
                    "injected list error".to_string(),
                ))
            })
        }

        fn matching<'a>(
            &'a self,
            _tenant_id: &'a str,
            _nats_subject: &'a str,
            _payload: &'a serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<trogon_automations::Automation>, trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(vec![]) })
        }
    }

    /// Promise store that delegates all operations to an inner [`MockPromiseStore`]
    /// except `update_promise` — when the updated promise has status
    /// `PermanentFailed`, the write returns an immediate error instead.
    ///
    /// This simulates the KV write failing after the promise was already
    /// CAS-claimed (status `Running`), verifying that recovery continues without
    /// panicking and leaves the promise in `Running` state.
    struct FailsPermanentFailedUpdateStore {
        inner: crate::promise_store::mock::MockPromiseStore,
    }

    impl crate::promise_store::PromiseRepository for FailsPermanentFailedUpdateStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<crate::promise_store::PromiseEntry>, crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            if promise.status == crate::promise_store::PromiseStatus::PermanentFailed {
                return Box::pin(async {
                    Err(crate::promise_store::PromiseStoreError(
                        "injected: update_promise fails on PermanentFailed".to_string(),
                    ))
                });
            }
            self.inner.update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<String>, crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<crate::promise_store::AgentPromise>, crate::promise_store::PromiseStoreError>> + Send + 'a>>
        {
            self.inner.list_running(tenant_id)
        }
    }

    /// When `automation_store.list()` returns an error during startup recovery,
    /// the affected promise must be skipped (kept in `Running`) rather than
    /// marked `PermanentFailed`.
    ///
    /// Rationale: a transient KV error looks identical to "no automations", so
    /// treating an error as an empty list would incorrectly cycle promises until
    /// the 24 h TTL.  The fix is to propagate the error and retry on next restart.
    #[tokio::test]
    async fn recover_automation_list_error_skips_promise() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let inner = MockPromiseStore::new();
        let mut p = make_stale_promise("p-list-err");
        p.automation_id = "some-auto-id".to_string();
        inner.insert_promise(p);
        let store = Arc::new(inner);
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let (_, run_store, _container) = make_all_stores().await;
        let auto_store = Arc::new(ErrorListAutomationStore);
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .get_promise("acme", "p-list-err")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "automation_store.list() error must skip the promise, not mark it PermanentFailed"
        );
    }

    /// When the `update_promise` that marks a deleted-automation promise as
    /// `PermanentFailed` returns an error, recovery must log and continue
    /// without panicking — the promise stays in `Running` and is retried on
    /// the next restart.
    #[tokio::test]
    async fn recover_automation_deleted_permanent_failed_write_error_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-del-write-err");
        p.automation_id = "deleted-auto-id".to_string();
        inner.insert_promise(p);
        let store = Arc::new(FailsPermanentFailedUpdateStore { inner });
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        // Real AutomationStore — empty, so the automation is "deleted".
        let (auto_store, run_store, _container) = make_all_stores().await;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        // The PermanentFailed write failed → promise was NOT written to PermanentFailed.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-del-write-err")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_ne!(
            p.status,
            PromiseStatus::PermanentFailed,
            "update_promise error on PermanentFailed write must not crash recovery"
        );
    }

    /// When the `update_promise` that marks an unknown-subject promise as
    /// `PermanentFailed` returns an error, recovery must log and continue
    /// without panicking — the promise stays in `Running` and is retried on
    /// the next restart.
    #[tokio::test]
    async fn recover_unknown_subject_permanent_failed_write_error_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-unk-write-err");
        p.automation_id = String::new();
        p.nats_subject = "completely.unknown.subject".to_string();
        inner.insert_promise(p);
        let store = Arc::new(FailsPermanentFailedUpdateStore { inner });
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let (auto_store, run_store, _container) = make_all_stores().await;
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .inner
            .get_promise("acme", "p-unk-write-err")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_ne!(
            p.status,
            PromiseStatus::PermanentFailed,
            "update_promise error on PermanentFailed write must not crash recovery"
        );
    }

    // ── run_store.record() error paths ────────────────────────────────────────

    /// `RunRepository` implementation whose `record()` always returns an error.
    ///
    /// Used to verify that a failing `run_store.record()` call is treated as a
    /// non-fatal warning — the automation task and the recovery loop must both
    /// complete without panicking.
    #[derive(Clone)]
    struct ErrorRecordRunStore;

    impl trogon_automations::RunRepository for ErrorRecordRunStore {
        fn record<'a>(
            &'a self,
            _run: &'a trogon_automations::RunRecord,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(trogon_automations::store::StoreError(
                    "injected record error".to_string(),
                ))
            })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
            _automation_id: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<trogon_automations::RunRecord>, trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(vec![]) })
        }

        fn stats<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<trogon_automations::RunStats, trogon_automations::store::StoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(trogon_automations::RunStats {
                    total: 0,
                    successful_7d: 0,
                    failed_7d: 0,
                })
            })
        }
    }

    /// When `run_store.record()` fails inside `dispatch_automations`, the warning
    /// must be logged but the function must still return normally — a record
    /// failure must never abort the automation or propagate to the caller.
    #[tokio::test]
    async fn dispatch_automations_run_record_error_is_logged_not_fatal() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "done"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        let promise_store: Arc<dyn PromiseRepository> = Arc::new(MockPromiseStore::new());
        let payload = bytes::Bytes::from_static(b"{}");

        // `record()` always errors — must not propagate.
        dispatch_automations(
            &agent,
            &Arc::new(ErrorRecordRunStore),
            &promise_store,
            "github.1",
            vec![make_automation("rec-err-auto")],
            "github.push",
            &payload,
        )
        .await;

        mock.assert_hits_async(1).await;
    }

    /// When `run_store.record()` fails inside the startup recovery loop, the
    /// warning must be logged but recovery must complete without panicking.
    #[tokio::test]
    async fn recover_automation_run_record_error_is_logged_not_fatal() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovered"}]
                }));
        });

        // Insert the automation so recovery finds it (non-empty list).
        let (auto_store, _, _container) = make_all_stores().await;
        let auto = make_automation("run-rec-auto");
        auto_store.put(&auto).await.expect("put automation");

        // Stale promise that belongs to the automation above.
        let store = Arc::new(MockPromiseStore::new());
        let mut p = make_stale_promise("p-run-rec");
        p.automation_id = auto.id.clone();
        p.nats_subject = "github.push".to_string();
        store.insert_promise(p);
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let agent = make_agent(&server.base_url());

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &Arc::new(ErrorRecordRunStore),
            "acme",
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        // Must complete without panic even though record() always errors.
        handle.await.expect("recovery task must not panic");
    }

    /// Startup recovery happy path: when a stale promise references an
    /// automation that still exists, the agent runs to completion and the
    /// promise is marked `Resolved`.
    ///
    /// Symmetric counterpart to `recover_automation_deleted_marks_permanent_failed`.
    #[tokio::test]
    async fn recover_automation_run_completes_and_marks_promise_resolved() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovered"}]
                }));
        });

        let agent = make_agent(&server.base_url());
        // agent.tenant_id = "test" — keep everything under "test" so the agent's
        // internal KV writes (get_promise / update_promise Resolved) target the
        // same key that prepare_agent_with_promise claims.

        let (auto_store, run_store, _container) = make_all_stores().await;
        let mut auto = make_automation("happy-auto");
        auto.tenant_id = agent.tenant_id.clone(); // "test"
        auto_store.put(&auto).await.expect("put automation");

        // Stale promise: tenant_id matches the agent so the terminal Resolved
        // write lands at the correct KV key.
        let store = Arc::new(MockPromiseStore::new());
        let mut p = make_stale_promise("p-happy-rec");
        p.tenant_id = agent.tenant_id.clone(); // "test"
        p.automation_id = auto.id.clone();
        p.nats_subject = "github.push".to_string();
        store.insert_promise(p);
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            &agent.tenant_id, // "test"
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .get_promise(&agent.tenant_id, "p-happy-rec")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "automation found + run succeeds → promise must be marked Resolved; got {:?}",
            p.status
        );
    }

    // ── Additional mock stores ────────────────────────────────────────────────

    /// Captures every `RunRecord` passed to `record()` for later inspection.
    ///
    /// Avoids depending on `RunStore.list()` internals and NATS containers in
    /// tests that only need to verify which records were persisted.
    #[derive(Clone)]
    struct CapturingRunStore {
        records: Arc<std::sync::Mutex<Vec<trogon_automations::RunRecord>>>,
    }

    impl CapturingRunStore {
        fn new() -> Self {
            Self {
                records: Arc::new(std::sync::Mutex::new(vec![])),
            }
        }

        fn snapshot(&self) -> Vec<trogon_automations::RunRecord> {
            self.records.lock().unwrap().clone()
        }
    }

    impl trogon_automations::RunRepository for CapturingRunStore {
        fn record<'a>(
            &'a self,
            run: &'a trogon_automations::RunRecord,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), trogon_automations::store::StoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.records.lock().unwrap().push(run.clone());
            Box::pin(async { Ok(()) })
        }

        fn list<'a>(
            &'a self,
            _tenant_id: &'a str,
            _automation_id: Option<&'a str>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<trogon_automations::RunRecord>,
                            trogon_automations::store::StoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }

        fn stats<'a>(
            &'a self,
            _tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            trogon_automations::RunStats,
                            trogon_automations::store::StoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(trogon_automations::RunStats {
                    total: 0,
                    successful_7d: 0,
                    failed_7d: 0,
                })
            })
        }
    }

    /// Promise store where every `update_promise` call fails immediately.
    ///
    /// Used to verify that `prepare_agent_with_promise` returns `None` (CAS
    /// claim rejected) so callers skip the automation without running it.
    struct AlwaysFailsUpdateStore {
        inner: crate::promise_store::mock::MockPromiseStore,
    }

    impl crate::promise_store::PromiseRepository for AlwaysFailsUpdateStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            _tenant_id: &'a str,
            _promise_id: &'a str,
            _promise: &'a crate::promise_store::AgentPromise,
            _revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Err(crate::promise_store::PromiseStoreError(
                    "injected: update always fails".to_string(),
                ))
            })
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    /// Promise store that returns `Ok(None)` from `get_promise` starting at
    /// call number `none_from_call` (0-indexed).  Earlier calls delegate to the
    /// inner store.
    ///
    /// Used to exercise the `Ok(Ok(None)) => {}` arm in the
    /// `PermanentFailed`-marking phase: call 0 (recovery re-fetch) and call 1
    /// (inside `prepare_agent_with_promise`) return `Some`; call 2 (the marking
    /// `get_promise`) returns `None` — the promise expired in that window.
    struct GetReturnsNoneAfterNthCallStore {
        inner: crate::promise_store::mock::MockPromiseStore,
        get_count: Arc<std::sync::atomic::AtomicUsize>,
        none_from_call: usize,
    }

    impl GetReturnsNoneAfterNthCallStore {
        fn new(
            none_from_call: usize,
            inner: crate::promise_store::mock::MockPromiseStore,
        ) -> Self {
            Self {
                inner,
                get_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                none_from_call,
            }
        }
    }

    impl crate::promise_store::PromiseRepository for GetReturnsNoneAfterNthCallStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            let n = self
                .get_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n >= self.none_from_call {
                return Box::pin(async { Ok(None) });
            }
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    /// Promise store where `update_promise` hangs (returns `pending()`)
    /// starting at call number `hang_from_call` (0-indexed).
    ///
    /// Used with `tokio::time::pause()` + `tokio::time::advance` to trigger the
    /// `tokio::time::timeout` wrapper around the `PermanentFailed`-marking
    /// `update_promise`, exercising the `Err(_)` (timeout) arm.
    /// `hang_from_call = 1` is the common setup: call 0 is the CAS claim
    /// (succeeds), call 1 is the marking write (hangs → timeout).
    struct UpdateHangsAfterNthCallStore {
        inner: crate::promise_store::mock::MockPromiseStore,
        update_count: Arc<std::sync::atomic::AtomicUsize>,
        hang_from_call: usize,
    }

    impl UpdateHangsAfterNthCallStore {
        fn new(
            hang_from_call: usize,
            inner: crate::promise_store::mock::MockPromiseStore,
        ) -> Self {
            Self {
                inner,
                update_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                hang_from_call,
            }
        }
    }

    impl crate::promise_store::PromiseRepository for UpdateHangsAfterNthCallStore {
        fn get_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<crate::promise_store::PromiseEntry>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_promise(tenant_id, promise_id)
        }

        fn put_promise<'a>(
            &'a self,
            promise: &'a crate::promise_store::AgentPromise,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.put_promise(promise)
        }

        fn update_promise<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            promise: &'a crate::promise_store::AgentPromise,
            revision: u64,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<u64, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            let n = self
                .update_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n >= self.hang_from_call {
                return Box::pin(async {
                    std::future::pending::<
                        Result<u64, crate::promise_store::PromiseStoreError>,
                    >()
                    .await
                });
            }
            self.inner
                .update_promise(tenant_id, promise_id, promise, revision)
        }

        fn get_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<String>, crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.get_tool_result(tenant_id, promise_id, cache_key)
        }

        fn put_tool_result<'a>(
            &'a self,
            tenant_id: &'a str,
            promise_id: &'a str,
            cache_key: &'a str,
            result: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), crate::promise_store::PromiseStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner
                .put_tool_result(tenant_id, promise_id, cache_key, result)
        }

        fn list_running<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::promise_store::AgentPromise>,
                            crate::promise_store::PromiseStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.list_running(tenant_id)
        }
    }

    /// Empty automation store — `list()` always returns `Ok(vec![])`.
    ///
    /// Used in paused-clock tests to avoid spinning up a NATS container;
    /// `async_nats::connect` uses tokio time internally and would break
    /// if the clock is paused before the connection is established.
    #[derive(Clone)]
    struct EmptyAutomationStore;

    impl trogon_automations::AutomationRepository for EmptyAutomationStore {
        fn put<'a>(
            &'a self,
            _: &'a trogon_automations::Automation,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), trogon_automations::store::StoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(()) })
        }

        fn get<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Option<trogon_automations::Automation>,
                            trogon_automations::store::StoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(None) })
        }

        fn delete<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<(), trogon_automations::store::StoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(()) })
        }

        fn list<'a>(
            &'a self,
            _: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<trogon_automations::Automation>,
                            trogon_automations::store::StoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }

        fn matching<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: &'a serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<trogon_automations::Automation>,
                            trogon_automations::store::StoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(vec![]) })
        }
    }

    // ── Tests for remaining gaps ──────────────────────────────────────────────

    /// When `handlers::run_automation` returns `Err` during startup recovery
    /// (e.g. a non-retryable 400 from Anthropic), a `RunRecord` with
    /// `status = RunStatus::Failed` must be persisted.  Recovery must continue
    /// normally — the error is logged, not propagated.
    #[tokio::test]
    async fn recover_automation_run_fails_stores_failed_run_record() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use trogon_automations::RunStatus;

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "error": {"type": "invalid_request_error", "message": "injected 400"}
                }));
        });

        let agent = make_agent(&server.base_url()); // tenant_id = "test"

        let mut auto = make_automation("fail-rec");
        auto.tenant_id = agent.tenant_id.clone(); // "test"

        let store = Arc::new(MockPromiseStore::new());
        let mut p = make_stale_promise("p-run-fail-rec");
        p.tenant_id = agent.tenant_id.clone(); // "test"
        p.automation_id = auto.id.clone();
        p.nats_subject = "github.push".to_string();
        store.insert_promise(p);
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let run_store = Arc::new(CapturingRunStore::new());

        // Real AutomationStore so the automation is found and the run path is taken.
        let (auto_store, _, _container) = make_all_stores().await;
        auto_store.put(&auto).await.expect("put automation");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("spawn handle must be returned when stale promises exist");

        handle.await.expect("recovery task must not panic");

        let records = run_store.snapshot();
        assert_eq!(records.len(), 1, "exactly one run record must be created");
        assert_eq!(
            records[0].status,
            RunStatus::Failed,
            "failed automation must produce RunRecord with status Failed"
        );
        assert_eq!(records[0].automation_id, auto.id);
    }

    /// When `handlers::run_automation` returns `Err` inside
    /// `dispatch_automations` (e.g. a non-retryable 400 from Anthropic), a
    /// `RunRecord` with `status = RunStatus::Failed` must be persisted.
    /// `dispatch_automations` must return normally.
    #[tokio::test]
    async fn dispatch_automations_run_fails_stores_failed_run_record() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::PromiseRepository;
        use trogon_automations::RunStatus;

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "error": {"type": "invalid_request_error", "message": "injected 400"}
                }));
        });

        let agent = make_agent(&server.base_url());
        let run_store = Arc::new(CapturingRunStore::new());
        let promise_store: Arc<dyn PromiseRepository> = Arc::new(MockPromiseStore::new());
        let payload = bytes::Bytes::from_static(b"{}");
        let auto = make_automation("fail-dispatch");

        dispatch_automations(
            &agent,
            &run_store,
            &promise_store,
            "github.1",
            vec![auto.clone()],
            "github.push",
            &payload,
        )
        .await;

        let records = run_store.snapshot();
        assert_eq!(records.len(), 1, "exactly one run record must be created");
        assert_eq!(
            records[0].status,
            RunStatus::Failed,
            "failed dispatch automation must produce RunRecord with status Failed"
        );
        assert_eq!(records[0].automation_id, auto.id);
    }

    /// When `prepare_agent_with_promise` returns `None` (CAS claim rejected by
    /// `AlwaysFailsUpdateStore`), the automation task must return without calling
    /// `run_automation` — no `RunRecord` is persisted.
    #[tokio::test]
    async fn dispatch_automations_cas_claim_lost_skips_automation() {
        use crate::promise_store::mock::MockPromiseStore;
        use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus};

        let auto = make_automation("cas-dispatch");
        // Pre-insert a Running promise with the promise_id dispatch_automations
        // will compute: "{prefix}.{auto.id}" = "github.1.id-cas-dispatch".
        let inner = MockPromiseStore::new();
        inner.insert_promise(AgentPromise {
            id: format!("github.1.{}", auto.id),
            tenant_id: "test".to_string(), // make_agent uses "test"
            automation_id: auto.id.clone(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "other-worker".to_string(),
            claimed_at: trogon_automations::now_unix(),
            trigger: serde_json::json!({}),
            nats_subject: "github.push".to_string(),
            system_prompt: None,
        });
        // AlwaysFailsUpdateStore: every update_promise call fails → CAS claim lost.
        let store = Arc::new(AlwaysFailsUpdateStore { inner });
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let run_store = Arc::new(CapturingRunStore::new());
        let payload = bytes::Bytes::from_static(b"{}");
        // Using unreachable base URL — any attempted LLM call would cause a
        // connection error, producing a run record with status Failed.  An empty
        // records snapshot is the definitive signal that no automation ran.
        let agent = make_agent("http://127.0.0.1:1");

        dispatch_automations(
            &agent,
            &run_store,
            &promise_store,
            "github.1",
            vec![auto],
            "github.push",
            &payload,
        )
        .await;

        assert!(
            run_store.snapshot().is_empty(),
            "CAS claim lost → automation must not run → no run record created"
        );
    }

    /// When the `get_promise` call inside the `PermanentFailed`-marking phase
    /// (automation-deleted path) returns `None`, recovery must continue without
    /// panicking — the `Ok(Ok(None)) => {}` arm is hit silently.
    #[tokio::test]
    async fn recover_automation_deleted_get_promise_none_in_marking_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-del-none");
        p.automation_id = "gone-auto-none".to_string();
        inner.insert_promise(p);

        // Calls 0 and 1 return Some (re-fetch + prepare_agent_with_promise).
        // Call 2 (marking get_promise) returns None.
        let store = Arc::new(GetReturnsNoneAfterNthCallStore::new(2, inner));
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        // Real NATS (empty) so the automation is "deleted" (not found in list).
        let (auto_store, _, _container) = make_all_stores().await;
        let run_store = Arc::new(CapturingRunStore::new());
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle");

        handle.await.expect("recovery task must not panic");

        // The marking write was skipped (get returned None) — promise stays Running.
        let (p, _) = store
            .inner
            .get_promise("acme", "p-del-none")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "marking skipped (get_promise returned None) → promise stays Running"
        );
    }

    /// When the `get_promise` call inside the `PermanentFailed`-marking phase
    /// (unknown-subject path) returns `None`, recovery must continue without
    /// panicking.
    #[tokio::test]
    async fn recover_unknown_subject_get_promise_none_in_marking_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-unk-none");
        p.automation_id = String::new(); // built-in handler path
        p.nats_subject = "completely.unknown.subject".to_string();
        inner.insert_promise(p);

        let store = Arc::new(GetReturnsNoneAfterNthCallStore::new(2, inner));
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let (auto_store, _, _container) = make_all_stores().await;
        let run_store = Arc::new(CapturingRunStore::new());
        let agent = make_agent("http://127.0.0.1:1");

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle");

        handle.await.expect("recovery task must not panic");

        let (p, _) = store
            .inner
            .get_promise("acme", "p-unk-none")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "unknown-subject marking skipped (get_promise None) → promise stays Running"
        );
    }

    /// When the `update_promise` call in the `PermanentFailed`-marking phase
    /// (automation-deleted path) times out, recovery must log a warning and
    /// continue — promise stays Running, no panic.
    #[tokio::test]
    async fn recover_automation_deleted_update_promise_timeout_in_marking_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use std::time::Duration;

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-del-upd-timeout");
        p.automation_id = "gone-auto-upd-timeout".to_string();
        inner.insert_promise(p);

        // Call 0 (CAS claim in prepare_agent_with_promise): delegates → succeeds.
        // Call 1 (PermanentFailed marking write): hangs → NATS_KV_TIMEOUT fires.
        let store = Arc::new(UpdateHangsAfterNthCallStore::new(1, inner));
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        // Fully in-memory stores: pausing the clock before NATS connect would
        // break async_nats' internal tokio::time::timeout calls.
        let auto_store = Arc::new(EmptyAutomationStore); // empty → automation "deleted"
        let run_store = Arc::new(ErrorRecordRunStore); // record() is never reached here
        let agent = make_agent("http://127.0.0.1:1");

        // Pause the clock AFTER all in-memory setup (no NATS containers to worry about).
        tokio::time::pause();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle");

        // Advance past NATS_KV_TIMEOUT to trigger the update_promise timeout.
        let (join_result, _) = tokio::join!(
            handle,
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1),
            ),
        );
        join_result.expect("recovery task must not panic");

        let (p, _) = store
            .inner
            .get_promise("acme", "p-del-upd-timeout")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "update_promise timeout in marking → promise stays Running (not PermanentFailed)"
        );
    }

    /// When the `update_promise` call in the `PermanentFailed`-marking phase
    /// (unknown-subject path) times out, recovery must log a warning and
    /// continue.
    #[tokio::test]
    async fn recover_unknown_subject_update_promise_timeout_in_marking_continues() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};
        use std::time::Duration;

        let inner = crate::promise_store::mock::MockPromiseStore::new();
        let mut p = make_stale_promise("p-unk-upd-timeout");
        p.automation_id = String::new();
        p.nats_subject = "completely.unknown.subject".to_string();
        inner.insert_promise(p);

        let store = Arc::new(UpdateHangsAfterNthCallStore::new(1, inner));
        let promise_store: Arc<dyn PromiseRepository> = Arc::clone(&store) as _;

        let auto_store = Arc::new(EmptyAutomationStore);
        let run_store = Arc::new(ErrorRecordRunStore);
        let agent = make_agent("http://127.0.0.1:1");

        tokio::time::pause();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_store,
            &auto_store,
            &run_store,
            "acme",
        )
        .await
        .expect("spawn handle");

        let (join_result, _) = tokio::join!(
            handle,
            tokio::time::advance(
                crate::agent_loop::NATS_KV_TIMEOUT + Duration::from_millis(1),
            ),
        );
        join_result.expect("recovery task must not panic");

        let (p, _) = store
            .inner
            .get_promise("acme", "p-unk-upd-timeout")
            .await
            .unwrap()
            .expect("promise must still exist");
        assert_eq!(
            p.status,
            PromiseStatus::Running,
            "unknown-subject update_promise timeout → promise stays Running"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Real NATS KV integration tests
    //
    // All tests below use a real NATS JetStream container (via testcontainers)
    // and the real `PromiseStore` implementation — no mocks.
    // ══════════════════════════════════════════════════════════════════════════

    /// Start a throwaway NATS container and open a real `PromiseStore`,
    /// `AutomationStore`, `RunStore`, and a raw `JetStream` context (needed for
    /// the `spawn_heartbeat` stream setup).
    ///
    /// Returns the stores and an opaque drop guard — keep the guard alive for
    /// the duration of the test or the container will be stopped.
    async fn make_all_stores_with_promise() -> (
        Arc<crate::promise_store::PromiseStore>,
        Arc<trogon_automations::AutomationStore>,
        Arc<trogon_automations::RunStore>,
        async_nats::jetstream::Context,
        impl Drop,
    ) {
        use testcontainers_modules::{
            nats::Nats,
            testcontainers::{ImageExt, runners::AsyncRunner},
        };
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("NATS container");
        let port = container.get_host_port_ipv4(4222).await.expect("NATS port");
        let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .expect("NATS connect");
        let js = async_nats::jetstream::new(nats);
        let promise_store = Arc::new(
            crate::promise_store::PromiseStore::open(&js)
                .await
                .expect("PromiseStore::open"),
        );
        let auto_store = Arc::new(
            trogon_automations::AutomationStore::open(&js)
                .await
                .expect("AutomationStore"),
        );
        let run_store = Arc::new(
            trogon_automations::RunStore::open(&js)
                .await
                .expect("RunStore"),
        );
        (promise_store, auto_store, run_store, js, container)
    }

    // ── prepare_agent_with_promise (real KV) ─────────────────────────────────

    /// A new trigger (no prior KV entry) must cause `prepare_agent_with_promise`
    /// to create a `Running` promise in real NATS KV and return a wired agent.
    #[tokio::test]
    async fn real_kv_prepare_agent_creates_promise() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (ps, _, _, _, _c) = make_all_stores_with_promise().await;
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &promise_repo,
            "test",
            "p-real-new",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_some(), "must return an agent for a new promise");
        assert_eq!(
            result.unwrap().promise_id.as_deref(),
            Some("p-real-new"),
            "returned agent must be wired with the promise id"
        );

        let (p, _) = ps
            .get_promise("test", "p-real-new")
            .await
            .unwrap()
            .expect("promise must exist in real KV after creation");
        assert_eq!(p.status, PromiseStatus::Running);
        assert_eq!(p.nats_subject, "github.pull_request");
        assert_eq!(p.tenant_id, "test");
    }

    /// An existing `Running` promise must be CAS-claimed: `worker_id` is
    /// replaced with the current worker's id and written back to real KV.
    #[tokio::test]
    async fn real_kv_prepare_agent_cas_claims_running_promise() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (ps, _, _, _, _c) = make_all_stores_with_promise().await;

        // Pre-insert a Running promise that appears to belong to a crashed worker.
        let mut p = sample_promise("p-real-run", PromiseStatus::Running);
        p.tenant_id = "test".to_string();
        p.worker_id = "crashed-worker".to_string();
        ps.put_promise(&p).await.unwrap();

        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &promise_repo,
            "test",
            "p-real-run",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(result.is_some(), "must return agent when claiming a Running promise");

        // KV must reflect the CAS claim: worker_id must have changed.
        let (claimed, _) = ps
            .get_promise("test", "p-real-run")
            .await
            .unwrap()
            .expect("promise must still exist after CAS claim");
        assert_ne!(
            claimed.worker_id, "crashed-worker",
            "worker_id must be updated by the CAS claim"
        );
        assert_eq!(
            claimed.status,
            PromiseStatus::Running,
            "status must remain Running after claim"
        );
    }

    /// A `Resolved` promise must cause `prepare_agent_with_promise` to return
    /// `None` — NATS redelivered the trigger but the run already completed.
    #[tokio::test]
    async fn real_kv_prepare_agent_skips_resolved_promise() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let (ps, _, _, _, _c) = make_all_stores_with_promise().await;

        let mut p = sample_promise("p-real-res", PromiseStatus::Resolved);
        p.tenant_id = "test".to_string();
        ps.put_promise(&p).await.unwrap();

        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        let agent = make_agent("http://127.0.0.1:1");

        let result = super::prepare_agent_with_promise(
            &agent,
            &promise_repo,
            "test",
            "p-real-res",
            "",
            "github.pull_request",
            &serde_json::json!({}),
        )
        .await;

        assert!(
            result.is_none(),
            "Resolved promise must return None — no re-run on NATS redelivery"
        );
    }

    // ── dispatch_automations (real KV) ────────────────────────────────────────

    /// A successful automation run must leave the promise `Resolved` in real
    /// NATS KV.  Verifies the full write path: create promise → run agent →
    /// mark terminal status.
    #[tokio::test]
    async fn real_kv_dispatch_automations_resolves_promise() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "done"}]
                }));
        });

        let (ps, _, run_store, _, _c) = make_all_stores_with_promise().await;
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        let agent = make_agent(&server.base_url());

        let mut auto = make_automation("real-dispatch-auto");
        auto.tenant_id = agent.tenant_id.clone(); // "test"

        let payload = bytes::Bytes::from_static(b"{}");

        dispatch_automations(
            &agent,
            &run_store,
            &promise_repo,
            "github.42",
            vec![auto.clone()],
            "github.push",
            &payload,
        )
        .await;

        // dispatch_automations constructs promise_id as "{prefix}.{auto.id}".
        let promise_id = format!("github.42.{}", auto.id);
        let (p, _) = ps
            .get_promise(&agent.tenant_id, &promise_id)
            .await
            .unwrap()
            .expect("promise must exist in real KV after dispatch");
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "successful run must mark promise Resolved in real KV; got {:?}",
            p.status
        );
    }

    // ── recover_stale_promises (real KV) ─────────────────────────────────────

    /// A stale `Running` promise for an existing automation must be recovered:
    /// the agent runs against real KV and the promise is marked `Resolved`.
    #[tokio::test]
    async fn real_kv_recover_stale_resolves_automation() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovered"}]
                }));
        });

        let (ps, auto_store, run_store, _, _c) = make_all_stores_with_promise().await;
        let agent = make_agent(&server.base_url());

        let mut auto = make_automation("stale-auto");
        auto.tenant_id = agent.tenant_id.clone();
        auto_store.put(&auto).await.expect("put automation");

        // Stale promise: Running, claimed 20 minutes ago, automation_id matches.
        let mut stale = make_stale_promise("stale.auto-id");
        stale.tenant_id = agent.tenant_id.clone();
        stale.automation_id = auto.id.clone();
        stale.nats_subject = "github.push".to_string();

        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        promise_repo.put_promise(&stale).await.unwrap();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_repo,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("must return a handle when stale promises exist");
        handle.await.expect("recovery task must not panic");

        let (p, _) = ps
            .get_promise(&agent.tenant_id, &stale.id)
            .await
            .unwrap()
            .expect("promise must still exist after recovery");
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "recovered automation run must mark promise Resolved; got {:?}",
            p.status
        );
    }

    // ── Crash-and-recovery simulation ─────────────────────────────────────────

    /// THE key durability test.
    ///
    /// **Scenario:** Worker A ran turn 1 of an automation and wrote a checkpoint
    /// (`iteration = 1`, `messages = [user_msg, assistant_turn1]`) to real NATS
    /// KV.  Worker A then crashed.  Worker B starts, finds the stale promise via
    /// `recover_stale_promises`, and must resume from the checkpoint rather than
    /// restarting from scratch.
    ///
    /// **Discriminator:** the Anthropic mock only matches requests whose body
    /// contains `"already did step one"` — the text from the checkpoint's
    /// assistant message.  If recovery resumes correctly the body carries the
    /// full message history, the mock is hit exactly once, and the promise ends
    /// up `Resolved`.  If recovery restarted from scratch the body would only
    /// carry the trigger and the mock would not match — causing an HTTP error,
    /// a `Failed` promise, and assertion failures.
    #[tokio::test]
    async fn real_kv_recovery_resumes_from_checkpoint_not_from_scratch() {
        use crate::agent_loop::{ContentBlock, Message};
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages")
                .body_contains("already did step one");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovery confirmed"}]
                }));
        });

        let (ps, auto_store, run_store, _, _c) = make_all_stores_with_promise().await;

        // Use max_iterations = 2 so the recovered run can make one more call.
        let agent = {
            let mut a = (*make_agent(&server.base_url())).clone();
            a.max_iterations = 2;
            Arc::new(a)
        };

        let mut auto = make_automation("crash-auto");
        auto.tenant_id = agent.tenant_id.clone();
        auto_store.put(&auto).await.expect("put automation");

        // Checkpoint written by worker A before it crashed:
        // – iteration = 1: one LLM turn already completed.
        // – messages: the full conversation history up to that turn.
        let promise_id = format!("crash.{}", auto.id);
        let stale = crate::promise_store::AgentPromise {
            id: promise_id.clone(),
            tenant_id: agent.tenant_id.clone(),
            automation_id: auto.id.clone(),
            status: PromiseStatus::Running,
            messages: vec![
                Message::user_text("run the automation"),
                Message::assistant(vec![ContentBlock::Text {
                    text: "already did step one".to_string(),
                }]),
            ],
            iteration: 1,
            worker_id: "crashed-worker-a".to_string(),
            claimed_at: trogon_automations::now_unix().saturating_sub(20 * 60),
            trigger: serde_json::json!({}),
            nats_subject: "github.push".to_string(),
            system_prompt: None,
        };

        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        promise_repo.put_promise(&stale).await.unwrap();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_repo,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("must return a handle for the stale promise");
        handle.await.expect("recovery task must not panic");

        // The mock must have been hit exactly once with the checkpoint messages
        // in the body — proving worker B resumed rather than restarted.
        mock.assert_hits_async(1).await;

        // The promise must be Resolved in real NATS KV.
        let (p, _) = ps
            .get_promise(&agent.tenant_id, &promise_id)
            .await
            .unwrap()
            .expect("promise must still exist in real KV after crash-and-recovery");
        assert_eq!(
            p.status,
            PromiseStatus::Resolved,
            "promise must be Resolved after successful crash-and-recovery; got {:?}",
            p.status
        );
    }

    // ── spawn_heartbeat (real JetStream) ──────────────────────────────────────

    /// `spawn_heartbeat` must return a non-finished `JoinHandle` and terminate
    /// cleanly when aborted.
    ///
    /// Also verifies that `AckKind::Progress` succeeds on a real JetStream
    /// message — the underlying call that every heartbeat tick makes.
    #[tokio::test]
    async fn spawn_heartbeat_sends_progress_ack_and_aborts_cleanly() {
        use async_nats::jetstream::{AckKind, consumer::{AckPolicy, pull}};
        use futures_util::StreamExt as _;
        use std::time::Duration;

        let (_, _, _, js, _c) = make_all_stores_with_promise().await;

        // Create a minimal stream for the heartbeat test.
        js.get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "HB_TEST".to_string(),
            subjects: vec!["hb.>".to_string()],
            ..Default::default()
        })
        .await
        .expect("create HB_TEST stream");

        // Publish one message so a consumer can fetch it.
        js.publish("hb.tick", bytes::Bytes::from_static(b"{}"))
            .await
            .expect("publish")
            .await
            .expect("server ack");

        // Create a pull consumer with explicit acks and a generous ack_wait.
        let stream = js.get_stream("HB_TEST").await.expect("get stream");
        let consumer: async_nats::jetstream::consumer::Consumer<pull::Config> = stream
            .get_or_create_consumer(
                "hb-consumer",
                pull::Config {
                    durable_name: Some("hb-consumer".to_string()),
                    ack_policy: AckPolicy::Explicit,
                    ack_wait: Duration::from_secs(30),
                    ..Default::default()
                },
            )
            .await
            .expect("create consumer");

        let mut messages = consumer.messages().await.expect("messages stream");
        let msg = messages.next().await.unwrap().unwrap();

        // Directly verify that `AckKind::Progress` works on a real JetStream
        // message — this is the exact call that spawn_heartbeat makes in its loop.
        msg.ack_with(AckKind::Progress)
            .await
            .expect("AckKind::Progress must succeed on a real JetStream message");

        // Start the heartbeat task.  The first actual Progress ack from the task
        // is sent after HEARTBEAT_INTERVAL_SECS (15 s); we don't wait that long.
        // We only verify the task lifecycle: starts running, aborts cleanly.
        let handle = super::spawn_heartbeat(msg);
        assert!(
            !handle.is_finished(),
            "heartbeat task must be running immediately after spawn"
        );

        // Abort — simulates the handler completing before the next tick.
        handle.abort();

        // Join with a short timeout to confirm the task terminates promptly.
        let outcome = tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("heartbeat task must terminate within 200 ms of abort");

        assert!(
            outcome.unwrap_err().is_cancelled(),
            "aborted heartbeat task must return a cancellation JoinError"
        );
    }

    // ── CAS race: concurrent claims (real NATS KV) ────────────────────────────

    /// When two workers hold the same KV revision and simultaneously attempt
    /// to `update_promise`, exactly one must succeed and the other must receive
    /// a CAS conflict error.
    ///
    /// The test reads the initial revision once (guaranteeing both workers
    /// share the same `rev`), then races two `update_promise` calls via
    /// `tokio::join!`. NATS KV's atomic revision check ensures at-most-one
    /// winner — this test verifies that the real NATS server enforces the
    /// invariant that `prepare_agent_with_promise` depends on.
    #[tokio::test]
    async fn real_kv_concurrent_update_promise_cas_only_one_winner() {
        use crate::promise_store::{AgentPromise, PromiseRepository, PromiseStatus};

        let (ps, _, _, _, _c) = make_all_stores_with_promise().await;
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();

        // Insert the initial promise and capture its revision.
        let initial = AgentPromise {
            id: "race-p".to_string(),
            tenant_id: "test".to_string(),
            automation_id: "race-auto".to_string(),
            status: PromiseStatus::Running,
            messages: vec![],
            iteration: 0,
            worker_id: "crashed-worker".to_string(),
            claimed_at: trogon_automations::now_unix().saturating_sub(5 * 60),
            trigger: serde_json::json!({}),
            nats_subject: "github.push".to_string(),
            system_prompt: None,
        };
        promise_repo.put_promise(&initial).await.unwrap();
        let (_, initial_rev) = promise_repo
            .get_promise("test", "race-p")
            .await
            .unwrap()
            .expect("promise must exist after put");

        // Both workers build their own claimed version from the shared revision.
        let mut claimed1 = initial.clone();
        claimed1.worker_id = "worker-1".to_string();
        let mut claimed2 = initial.clone();
        claimed2.worker_id = "worker-2".to_string();

        let pr1: Arc<dyn PromiseRepository> = ps.clone();
        let pr2: Arc<dyn PromiseRepository> = ps.clone();

        // Both race with the SAME revision — exactly one CAS update must succeed.
        let (r1, r2) = tokio::join!(
            pr1.update_promise("test", "race-p", &claimed1, initial_rev),
            pr2.update_promise("test", "race-p", &claimed2, initial_rev),
        );

        let wins = [r1.is_ok(), r2.is_ok()];
        assert_eq!(
            wins.iter().filter(|&&w| w).count(),
            1,
            "exactly one CAS update must succeed when both use the same revision; got {:?}",
            wins
        );

        // The KV entry must reflect the winner's worker_id.
        let (final_p, _) = promise_repo
            .get_promise("test", "race-p")
            .await
            .unwrap()
            .expect("promise must still exist after the race");
        let winner = if r1.is_ok() { "worker-1" } else { "worker-2" };
        assert_eq!(
            final_p.worker_id, winner,
            "KV must reflect the winning worker's claim"
        );
    }

    // ── Batch stale-promise recovery (real NATS KV) ───────────────────────────

    /// `recover_stale_promises` must recover ALL stale promises sequentially,
    /// not just the first one.
    ///
    /// Inserts 3 stale automations and their corresponding stale Running
    /// promises into real NATS KV, then calls `recover_stale_promises` and
    /// verifies every promise ends up `Resolved` after the recovery task
    /// completes. This test catches any early-exit bugs (e.g., `return` instead
    /// of `continue` inside the recovery loop).
    #[tokio::test]
    async fn real_kv_recover_three_stale_promises_all_resolved() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "recovered"}]
                }));
        });

        let (ps, auto_store, run_store, _, _c) = make_all_stores_with_promise().await;
        let agent = make_agent(&server.base_url());
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();

        let mut promise_ids = Vec::new();
        for i in 0..3usize {
            let mut auto = make_automation(&format!("batch-auto-{i}"));
            auto.tenant_id = agent.tenant_id.clone();
            auto_store.put(&auto).await.expect("put automation");

            let pid = format!("batch-stale-{i}");
            let mut stale = make_stale_promise(&pid);
            stale.tenant_id = agent.tenant_id.clone();
            stale.automation_id = auto.id.clone();
            stale.nats_subject = "github.push".to_string();
            promise_repo.put_promise(&stale).await.unwrap();
            promise_ids.push(pid);
        }

        let handle = super::recover_stale_promises(
            &agent,
            &promise_repo,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("must return a handle when 3 stale promises exist");
        handle.await.expect("recovery task must not panic");

        for pid in &promise_ids {
            let (p, _) = ps
                .get_promise(&agent.tenant_id, pid)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("promise {pid} must exist after batch recovery"));
            assert_eq!(
                p.status,
                PromiseStatus::Resolved,
                "promise {pid} must be Resolved after batch recovery; got {:?}",
                p.status
            );
        }
    }

    // ── Multi-automation parallel dispatch (real NATS KV) ─────────────────────

    /// `dispatch_automations` with 3 automations must create 3 independent
    /// promises in real NATS KV — one per automation, keyed as
    /// `{prefix}.{auto_id}` — and resolve all of them.
    ///
    /// Verifies: (a) the key construction is correct, (b) each promise carries
    /// the right `automation_id`, and (c) three parallel runs in the same KV
    /// bucket do not overwrite or corrupt each other's checkpoints.
    #[tokio::test]
    async fn real_kv_dispatch_three_automations_independent_promises() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "stop_reason": "end_turn",
                    "content": [{"type": "text", "text": "done"}]
                }));
        });

        let (ps, _, run_store, _, _c) = make_all_stores_with_promise().await;
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();
        let agent = make_agent(&server.base_url());

        let autos: Vec<_> = (0..3usize)
            .map(|i| {
                let mut a = make_automation(&format!("parallel-{i}"));
                a.tenant_id = agent.tenant_id.clone();
                a
            })
            .collect();
        let auto_ids: Vec<_> = autos.iter().map(|a| a.id.clone()).collect();

        dispatch_automations(
            &agent,
            &run_store,
            &promise_repo,
            "github.100",
            autos,
            "github.push",
            &bytes::Bytes::from_static(b"{}"),
        )
        .await;

        for auto_id in &auto_ids {
            let pid = format!("github.100.{auto_id}");
            let (p, _) = ps
                .get_promise(&agent.tenant_id, &pid)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("promise {pid} must exist in real KV after dispatch"));
            assert_eq!(
                p.status,
                PromiseStatus::Resolved,
                "automation promise {pid} must be Resolved; got {:?}",
                p.status
            );
            assert_eq!(
                p.automation_id, *auto_id,
                "promise automation_id must match the dispatched automation"
            );
        }
    }

    // ── recover_stale_promises: deleted automation (real NATS KV) ─────────────

    /// When a promise references an automation that no longer exists in the
    /// `AutomationStore`, `recover_stale_promises` must mark the promise
    /// `PermanentFailed` — not leave it `Running` indefinitely.
    ///
    /// Scenario: worker A crashed while running automation "gone-auto".
    /// Between the crash and startup recovery the automation was deleted from
    /// the store. Recovery must detect the missing automation and terminate
    /// the promise deterministically.
    #[tokio::test]
    async fn real_kv_recover_stale_orphaned_automation_marks_permanent_failed() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        // Mock responds to nothing — the agent must never reach Anthropic.
        let server = httpmock::MockServer::start_async().await;
        let unexpected = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("must not be called");
        });

        let (ps, auto_store, run_store, _, _c) = make_all_stores_with_promise().await;
        let agent = make_agent(&server.base_url());
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();

        // Stale promise pointing to an automation that was deleted.
        // The AutomationStore is empty — no automations stored.
        let mut stale = make_stale_promise("orphan-promise");
        stale.tenant_id = agent.tenant_id.clone();
        stale.automation_id = "id-deleted-auto".to_string();
        stale.nats_subject = "github.push".to_string();
        promise_repo.put_promise(&stale).await.unwrap();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_repo,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("must return a handle for the orphaned promise");
        handle.await.expect("recovery task must not panic");

        // Promise must be PermanentFailed — retrying would always reach the
        // same "no automation" branch and cycle until the 24-hour TTL.
        let (p, _) = ps
            .get_promise(&agent.tenant_id, "orphan-promise")
            .await
            .unwrap()
            .expect("promise must still exist in real KV");
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "orphaned automation promise must be PermanentFailed, not {:?}",
            p.status
        );

        // Anthropic must never have been called.
        unexpected.assert_hits_async(0).await;
    }

    // ── recover_stale_promises: unknown subject (real NATS KV) ────────────────

    /// A stale built-in-handler promise whose `nats_subject` no longer maps to
    /// any known handler must be marked `PermanentFailed` by startup recovery.
    ///
    /// Scenario: a promise was created for subject `"legacy.event.v1"` which
    /// was removed in a later deployment. Recovery hits the `other =>` arm of
    /// the subject dispatch and terminates the promise so it is not retried.
    #[tokio::test]
    async fn real_kv_recover_stale_unknown_subject_marks_permanent_failed() {
        use crate::promise_store::{PromiseRepository, PromiseStatus};

        let server = httpmock::MockServer::start_async().await;
        let unexpected = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/anthropic/v1/messages");
            then.status(500).body("must not be called");
        });

        let (ps, auto_store, run_store, _, _c) = make_all_stores_with_promise().await;
        let agent = make_agent(&server.base_url());
        let promise_repo: Arc<dyn PromiseRepository> = ps.clone();

        // Built-in-handler promise (empty automation_id) with an unrecognised subject.
        let mut stale = make_stale_promise("unknown-subj-promise");
        stale.tenant_id = agent.tenant_id.clone();
        stale.automation_id = String::new(); // built-in handler path
        stale.nats_subject = "legacy.event.v1".to_string(); // no handler exists
        promise_repo.put_promise(&stale).await.unwrap();

        let handle = super::recover_stale_promises(
            &agent,
            &promise_repo,
            &auto_store,
            &run_store,
            &agent.tenant_id,
        )
        .await
        .expect("must return a handle for the unknown-subject promise");
        handle.await.expect("recovery task must not panic");

        let (p, _) = ps
            .get_promise(&agent.tenant_id, "unknown-subj-promise")
            .await
            .unwrap()
            .expect("promise must still exist in real KV");
        assert_eq!(
            p.status,
            PromiseStatus::PermanentFailed,
            "unknown-subject promise must be PermanentFailed, not {:?}",
            p.status
        );

        unexpected.assert_hits_async(0).await;
    }
}
