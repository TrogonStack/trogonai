#![cfg_attr(coverage, feature(coverage_attribute))]
//! `trogon-acp` — ACP server that routes prompts through NATS to `trogon-acp-runner`.
//!
//! ## Architecture
//!
//! ```text
//! ACP client (Zed / editor)
//!        ↓ stdio (newline-delimited JSON-RPC)
//!   trogon-acp  [this binary]
//!     TrogonAcpAgent
//!       ├─ initialize / authenticate / new_session / set_session_mode
//!       │    handled locally (no NATS round-trip)
//!       ├─ load_session
//!       │    loads history from NATS KV, replays as session notifications
//!       └─ prompt / cancel
//!            Bridge<NatsClient>   ← acp-nats
//!               ↓↑ NATS Core (prompt publish / event subscribe)
//!         trogon-acp-runner      ← same process
//!           Runner               subscribes, runs AgentLoop, streams PromptEvents
//!              ↓
//!         Anthropic API (via trogon-secret-proxy)
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
//! | `AGENT_MODEL`      | `claude-opus-4-6`        | Claude model ID                    |
//! | `AGENT_MAX_ITERATIONS` | `10`                 | Max loop iterations per prompt     |

mod agent;

use std::sync::Arc;

use acp_nats::{AcpPrefix, Bridge, Config};
use agent_client_protocol::{
    AgentSideConnection, Client, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, ToolCallUpdate, ToolCallUpdateFields,
};
use async_nats::jetstream;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::info;

use acp_nats_agent::AgentSideNatsConnection;
use trogon_acp_runner::{GatewayConfig, PermissionReq, SessionStore, TrogonAgent};
use trogon_agent_core::agent_loop::AgentLoop;
use trogon_agent_core::tools::ToolContext;
use trogon_nats::NatsConfig;

#[cfg_attr(coverage, coverage(off))]
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
        thinking_budget: None,
    };

    let thinking_budget: Option<u32> = std::env::var("MAX_THINKING_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok());
    if let Some(budget) = thinking_budget {
        agent_loop.thinking_budget = Some(budget);
    }

    // ── Permission gate channel ───────────────────────────────────────────────
    // The Runner sends PermissionReq over this channel; the LocalSet task below
    // handles each request by calling conn.request_permission() on the ACP connection.

    let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(32);

    // ── Shared gateway config (set by authenticate(), consumed by Runner) ─────

    let gateway_config = std::sync::Arc::new(tokio::sync::RwLock::new(None::<GatewayConfig>));

    // ── Session store ─────────────────────────────────────────────────────────

    let store = trogon_acp_runner::SessionStore::open(&js).await?;

    // ── TrogonAgent (NATS subscriber + agent) ────────────────────────────────

    let ta = TrogonAgent::new(
        nats.clone(),
        store.clone(),
        agent_loop,
        acp_prefix.clone(),
        model.clone(),
        Some(perm_tx),
        gateway_config.clone(),
    );
    let acp_prefix_typed = AcpPrefix::new(&acp_prefix)?;
    let (_, runner_io_task) =
        AgentSideNatsConnection::new(ta, nats.clone(), acp_prefix_typed, |fut| {
            tokio::task::spawn_local(fut);
        });

    // ── Bridge (ACP prompt/cancel ↔ NATS) ────────────────────────────────────

    let (notification_tx, mut notification_rx) = mpsc::channel::<SessionNotification>(64);

    let nats_config = NatsConfig {
        servers: vec![nats_url],
        auth: trogon_nats::NatsAuth::None,
    };
    let config = Config::new(AcpPrefix::new(acp_prefix.clone())?, nats_config);

    let meter = opentelemetry::global::meter("trogon-acp");
    let bridge = Bridge::new(
        nats.clone(),
        trogon_std::time::SystemClock,
        &meter,
        config,
        notification_tx.clone(),
    );

    // ── TrogonAcpAgent (handles lifecycle locally, routes prompt/cancel via Bridge) ──

    let acp_agent = agent::TrogonAcpAgent::new(
        bridge,
        store.clone(),
        nats.clone(),
        acp_prefix,
        notification_tx.clone(),
        model.clone(),
        gateway_config,
    );

    // ── ACP connection over stdio ─────────────────────────────────────────────

    let local = LocalSet::new();

    local
        .run_until(async move {
            // AgentSideNatsConnection uses spawn_local internally — must run within a LocalSet.
            tokio::task::spawn_local(runner_io_task);

            let stdin = tokio::io::stdin().compat();
            let stdout = tokio::io::stdout().compat_write();

            let (conn, io_task) =
                AgentSideConnection::new(acp_agent, stdout, stdin, |fut| {
                    tokio::task::spawn_local(fut);
                });

            // Forward session notifications and handle permission requests in a single task
            // so `conn` (which is !Send) is only used within this LocalSet task.
            let perm_store = store.clone();
            let perm_notif_tx = notification_tx.clone();
            tokio::task::spawn_local(async move {
                loop {
                    tokio::select! {
                        maybe_notification = notification_rx.recv() => {
                            match maybe_notification {
                                Some(notification) => {
                                    if let Err(e) = conn.session_notification(notification).await {
                                        tracing::warn!(error = %e, "failed to forward session notification");
                                    }
                                }
                                None => break,
                            }
                        }
                        maybe_perm = perm_rx.recv() => {
                            if let Some(req) = maybe_perm {
                                handle_permission_request(&conn, req, &perm_store, &perm_notif_tx, &model).await;
                            }
                            // perm channel closing doesn't stop the loop
                        }
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

/// Returns `true` if `bypassPermissions` mode may be offered to the user.
/// Mirrors the TS `ALLOW_BYPASS = !IS_ROOT` constant: denied when running as root or sudo.
#[cfg_attr(coverage, coverage(off))]
fn allow_bypass() -> bool {
    if std::env::var("SUDO_UID").is_ok() || std::env::var("SUDO_USER").is_ok() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("Uid:\t")
                    && let Some(uid) = rest.split_whitespace().next()
                {
                    return uid != "0";
                }
            }
        }
    }
    true
}

/// Call `conn.request_permission` for a tool and send the allow/deny result
/// back to the Runner via the oneshot channel embedded in `req`.
///
/// Special cases:
/// - `ExitPlanMode`: presents mode-selection options instead of allow/deny.
/// - `allow_always`: saves the tool name to `allowed_tools` in the session store.
#[cfg_attr(coverage, coverage(off))]
async fn handle_permission_request(
    conn: &AgentSideConnection,
    req: PermissionReq,
    store: &SessionStore,
    notification_tx: &mpsc::Sender<SessionNotification>,
    default_model: &str,
) {
    // ── ExitPlanMode: let the user choose which mode to switch to ─────────────
    if req.tool_name == "ExitPlanMode" {
        let mut options = Vec::new();
        if allow_bypass() {
            options.push(PermissionOption::new(
                "bypassPermissions",
                "Yes, and bypass permissions",
                PermissionOptionKind::AllowAlways,
            ));
        }
        options.push(PermissionOption::new(
            "acceptEdits",
            "Yes, and auto-accept edits",
            PermissionOptionKind::AllowAlways,
        ));
        options.push(PermissionOption::new(
            "default",
            "Yes, and manually approve edits",
            PermissionOptionKind::AllowOnce,
        ));
        options.push(PermissionOption::new(
            "plan",
            "No, keep planning",
            PermissionOptionKind::RejectOnce,
        ));

        let fields = ToolCallUpdateFields::new().title("Exit Plan Mode".to_string());
        let tool_call = ToolCallUpdate::new(req.tool_call_id.clone(), fields);
        let perm_req = RequestPermissionRequest::new(req.session_id.clone(), tool_call, options);

        let (allowed, new_mode) = match conn.request_permission(perm_req).await {
            Ok(resp) => match resp.outcome {
                RequestPermissionOutcome::Selected(sel) => {
                    let id = sel.option_id.0.as_ref();
                    if id == "plan" {
                        (false, None)
                    } else {
                        (true, Some(id.to_string()))
                    }
                }
                _ => (false, None),
            },
            Err(e) => {
                tracing::warn!(error = %e, "ExitPlanMode permission request failed");
                (false, None)
            }
        };

        // Persist the mode change and notify the client
        if let Some(mode) = new_mode {
            use agent_client_protocol::{ConfigOptionUpdate, CurrentModeUpdate, SessionUpdate};
            if let Ok(mut state) = store.load(&req.session_id).await {
                state.mode = mode.clone();
                if let Err(e) = store.save(&req.session_id, &state).await {
                    tracing::warn!(error = %e, "failed to save session mode after ExitPlanMode");
                }
                let current_model = state.model.as_deref().unwrap_or(default_model);
                let config_options = agent::TrogonAcpAgent::<
                    async_nats::Client,
                    trogon_std::time::SystemClock,
                >::build_config_options(
                    &mode, current_model, allow_bypass()
                );
                let config_n = SessionNotification::new(
                    req.session_id.clone(),
                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
                );
                let _ = notification_tx.send(config_n).await;
            }
            let mode_n = SessionNotification::new(
                req.session_id.clone(),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(mode)),
            );
            let _ = notification_tx.send(mode_n).await;
        }

        let _ = req.response_tx.send(allowed);
        return;
    }

    // ── Standard tool permission request ──────────────────────────────────────
    let options = vec![
        PermissionOption::new(
            "allow_always",
            "Always Allow",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ];

    let fields = ToolCallUpdateFields::new()
        .title(req.tool_name.clone())
        .raw_input(req.tool_input.clone());
    let tool_call = ToolCallUpdate::new(req.tool_call_id.clone(), fields);

    let perm_req = RequestPermissionRequest::new(req.session_id.clone(), tool_call, options);

    let outcome = conn.request_permission(perm_req).await;

    let (allowed, save_always) = match outcome {
        Ok(resp) => match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                let id = sel.option_id.0.as_ref();
                let is_allowed = id == "allow" || id == "allow_always";
                let is_always = id == "allow_always";
                (is_allowed, is_always)
            }
            RequestPermissionOutcome::Cancelled => (false, false),
            _ => (false, false),
        },
        Err(e) => {
            tracing::warn!(error = %e, tool = %req.tool_name, "permission request failed — denying");
            (false, false)
        }
    };

    // Persist allow-always decision so ChannelPermissionChecker can auto-approve
    // future calls to this tool within the session.
    if save_always
        && let Ok(mut state) = store.load(&req.session_id).await
        && !state.allowed_tools.contains(&req.tool_name)
    {
        state.allowed_tools.push(req.tool_name.clone());
        if let Err(e) = store.save(&req.session_id, &state).await {
            tracing::warn!(error = %e, tool = %req.tool_name, "failed to save allowed_tools");
        }
    }

    let _ = req.response_tx.send(allowed);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize env-var tests — they mutate global process state.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn allow_bypass_false_when_sudo_uid_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("SUDO_USER") };
        unsafe { std::env::set_var("SUDO_UID", "1000") };
        let result = allow_bypass();
        unsafe { std::env::remove_var("SUDO_UID") };
        assert!(
            !result,
            "allow_bypass must return false when SUDO_UID is set"
        );
    }

    #[test]
    fn allow_bypass_false_when_sudo_user_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("SUDO_UID") };
        unsafe { std::env::set_var("SUDO_USER", "jorge") };
        let result = allow_bypass();
        unsafe { std::env::remove_var("SUDO_USER") };
        assert!(
            !result,
            "allow_bypass must return false when SUDO_USER is set"
        );
    }

    #[cfg_attr(coverage, coverage(off))]
    #[test]
    fn allow_bypass_true_when_no_sudo_vars_and_not_root() {
        let _lock = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("SUDO_UID") };
        unsafe { std::env::remove_var("SUDO_USER") };
        // Skip if actually running as root (uid 0)
        let running_as_root = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:\t"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .map(|uid| uid == "0")
            })
            .unwrap_or(false);
        if running_as_root {
            return;
        }
        assert!(
            allow_bypass(),
            "allow_bypass must return true for a normal (non-root) user"
        );
    }
}
