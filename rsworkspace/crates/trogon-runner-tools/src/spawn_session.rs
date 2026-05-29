use std::path::PathBuf;

use agent_client_protocol::{
    CloseSessionRequest, ContentBlock, NewSessionRequest, PromptRequest, SessionId,
    SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use acp_nats::{
    Bridge, FlushClient, JetStreamGetStream, JetStreamPublisher, PublishClient, RequestClient,
    SubscribeClient,
};
use trogon_nats::jetstream::{JsMessageOf, JsRequestMessage};
use trogon_std::time::GetElapsed;

pub use crate::worktree::create_worktree;

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
pub async fn create_sub_session<N, C, J>(
    bridge: &Bridge<N, C, J>,
    cwd: &str,
    mode: &str,
    permission_rules_text: Option<&str>,
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

    let req = NewSessionRequest::new(PathBuf::from(cwd)).meta(meta);

    let resp = agent_client_protocol::Agent::new_session(bridge, req)
        .await
        .map_err(|e| format!("new_session failed: {e}"))?;

    let session_id = resp.session_id.0.to_string();

    // For non-bypass modes, set the mode after creation (best-effort).
    if mode != "bypassPermissions" {
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
