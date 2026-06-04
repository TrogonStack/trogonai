use std::path::PathBuf;

use agent_client_protocol::{
    CloseSessionRequest, ContentBlock, NewSessionRequest, PromptRequest, SessionId,
    SetSessionConfigOptionRequest, SetSessionModeRequest, SetSessionModelRequest,
};
use acp_nats::{
    Bridge, FlushClient, JetStreamGetStream, JetStreamPublisher, PublishClient, RequestClient,
    SubscribeClient,
};
use trogon_nats::jetstream::{JsMessageOf, JsRequestMessage};
use trogon_std::time::GetElapsed;

pub use crate::worktree::create_worktree;

/// Permission and tool context propagated from a parent session into a sub-agent session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubSessionSpawnContext {
    /// Restrictive tool allowlist from a named subagent definition (empty = no restriction).
    pub tool_allowlist: Vec<String>,
    /// Parent auto-approve list (`allowed_tools`); not a restrictive allowlist.
    pub allowed_tools: Vec<String>,
    pub additional_read_dirs: Vec<String>,
    pub additional_roots: Vec<String>,
}

/// Open a new ACP session on the provided bridge.
///
/// `mode` controls the permission mode of the sub-session:
/// - `"bypassPermissions"`: sets `bypassPermissions=true` in the session meta.
/// - anything else: calls `set_session_mode(mode)` after creation (best-effort).
///
/// If `permission_rules_text` is `Some`, calls
/// `set_session_config_option("permissions", text)` after creation (best-effort).
///
/// Returns the new session id on success, or an error string on failure.
#[allow(clippy::too_many_arguments)]
pub async fn create_sub_session<N, C, J>(
    bridge: &Bridge<N, C, J>,
    cwd: &str,
    mode: &str,
    permission_rules_text: Option<&str>,
    system_prompt: Option<&str>,
    model: Option<&str>,
    spawn_ctx: Option<&SubSessionSpawnContext>,
) -> Result<String, String>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
{
    let mut meta = serde_json::Map::new();
    if mode == "bypassPermissions" {
        meta.insert("bypassPermissions".to_string(), serde_json::json!(true));
    }
    // Custom subagent system prompt overrides the default identity for this
    // sub-session (e.g. "You are a code reviewer …").
    if let Some(sp) = system_prompt.filter(|s| !s.is_empty()) {
        meta.insert(
            "systemPromptOverride".to_string(),
            serde_json::Value::String(sp.to_string()),
        );
    }
    if let Some(ctx) = spawn_ctx {
        if !ctx.tool_allowlist.is_empty() {
            meta.insert(
                "toolAllowlist".to_string(),
                serde_json::json!(ctx.tool_allowlist),
            );
        }
        if !ctx.allowed_tools.is_empty() {
            meta.insert(
                "allowedTools".to_string(),
                serde_json::json!(ctx.allowed_tools),
            );
        }
        if !ctx.additional_roots.is_empty() {
            meta.insert(
                "additionalRoots".to_string(),
                serde_json::json!(ctx.additional_roots),
            );
        }
        if !ctx.additional_read_dirs.is_empty() {
            meta.insert(
                "permissions".to_string(),
                serde_json::json!({
                    "additionalDirectories": ctx.additional_read_dirs,
                }),
            );
        }
    }

    let req = NewSessionRequest::new(PathBuf::from(cwd)).meta(meta);

    let resp = agent_client_protocol::Agent::new_session(bridge, req)
        .await
        .map_err(|e| format!("new_session failed: {e}"))?;

    let session_id = resp.session_id.0.to_string();

    // Apply the mode after creation for ALL modes, including `bypassPermissions`
    // (best-effort). The `bypassPermissions` meta flag above is only honored by
    // some runners' `new_session`, whereas every runner honors `set_session_mode`
    // — so this is what reliably propagates the parent's mode to the sub-session.
    if !mode.is_empty() {
        agent_client_protocol::Agent::set_session_mode(
            bridge,
            SetSessionModeRequest::new(session_id.clone(), mode.to_owned()),
        )
        .await
        .ok();
    }

    // Apply permission rules if provided (best-effort).
    if let Some(text) = permission_rules_text {
        agent_client_protocol::Agent::set_session_config_option(
            bridge,
            SetSessionConfigOptionRequest::new(session_id.clone(), "permissions", text),
        )
        .await
        .ok();
    }

    // Apply the subagent's model override if provided (best-effort).
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        agent_client_protocol::Agent::set_session_model(
            bridge,
            SetSessionModelRequest::new(session_id.clone(), m.to_owned()),
        )
        .await
        .ok();
    }

    Ok(session_id)
}

/// Send a prompt to an existing session on the provided bridge, then close it.
///
/// The prompt call is wrapped with a `timeout` safety-net. On timeout, returns
/// `Err("spawn_agent safety-net timeout after Xs")`. On prompt failure, returns
/// `Err("prompt failed: ...")`. On success, returns `Ok(())`.
///
/// The `close_session` call is always best-effort (errors are ignored).
pub async fn run_sub_session<N, C, J>(
    bridge: &Bridge<N, C, J>,
    session_id: &str,
    prompt: &str,
    timeout: std::time::Duration,
) -> Result<(), String>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
{
    let prompt_req = PromptRequest::new(
        SessionId::new(session_id),
        vec![ContentBlock::from(prompt)],
    );

    let prompt_result = tokio::time::timeout(
        timeout,
        agent_client_protocol::Agent::prompt(bridge, prompt_req),
    )
    .await;

    // Best-effort close — ignore errors so partial results are still returned.
    let _ = agent_client_protocol::Agent::close_session(
        bridge,
        CloseSessionRequest::new(SessionId::new(session_id)),
    )
    .await;

    match prompt_result {
        Err(_elapsed) => Err(format!(
            "spawn_agent safety-net timeout after {}s",
            timeout.as_secs()
        )),
        Ok(Err(e)) => Err(format!("prompt failed: {e}")),
        Ok(Ok(_)) => Ok(()),
    }
}
