//! Bridges in-process `PermissionReq` messages to the ACP client via NATS.

use std::time::Duration;

use acp_nats::{acp_prefix::AcpPrefix, client_proxy::NatsClientProxy, session_id::AcpSessionId};
use agent_client_protocol::{
    Client as AcpClient, PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use tracing::warn;
use trogon_nats::{FlushClient, PublishClient, RequestClient};

use crate::permission_rules::always_allow_key;
use crate::{PermissionReq, SessionStore};

/// Forward a single `PermissionReq` to the ACP client and send the allow/deny
/// decision back on the embedded oneshot channel.
pub async fn handle_permission_request_nats<S, N>(req: PermissionReq, nats: N, prefix: AcpPrefix, store: &S)
where
    S: SessionStore,
    N: RequestClient + PublishClient + FlushClient,
{
    let session_id = match AcpSessionId::new(&req.session_id) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                session_id = %req.session_id,
                "invalid session_id in permission request — denying"
            );
            let _ = req.response_tx.send(false);
            return;
        }
    };

    let proxy = NatsClientProxy::new(nats, session_id, prefix, Duration::from_secs(60));

    // For bash commands show "Always Allow `mkdir`" so the user knows the scope is
    // per-binary, not the entire bash tool.
    let always_key = always_allow_key(&req.tool_name, &req.tool_input);
    let always_label: std::borrow::Cow<str> = if let Some(bin) = always_key.strip_prefix("Bash:") {
        format!("Always Allow `{bin}`").into()
    } else {
        "Always Allow".into()
    };

    let options = vec![
        PermissionOption::new("allow_always", always_label.as_ref(), PermissionOptionKind::AllowAlways),
        PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
    ];

    let fields = ToolCallUpdateFields::new()
        .title(req.tool_name.clone())
        .raw_input(req.tool_input.clone());
    let tool_call = ToolCallUpdate::new(req.tool_call_id.clone(), fields);
    let perm_req = RequestPermissionRequest::new(req.session_id.clone(), tool_call, options);

    // MED-11: signal the checker that this request has reached the front of the
    // (sequential) bridge queue and the user is about to be prompted, so its
    // response timeout starts now rather than when it was first enqueued.
    let _ = req.started_tx.send(());

    let outcome = proxy.request_permission(perm_req).await;

    let (allowed, save_always) = match outcome {
        Ok(resp) => match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                let id = sel.option_id.0.as_ref();
                // Regular tool prompts return "allow"/"allow_always"/"reject".
                // ExitPlanMode prompts (tui_client::exit_plan_mode_options) instead
                // return "acceptEdits"/"default"/"bypassPermissions" — all meaning
                // "yes, leave plan mode" — or "plan" meaning "keep planning". Treat
                // every proceed id as allowed so an ExitPlanMode approval over the
                // NATS bridge isn't misread as a deny.
                let is_allowed = matches!(
                    id,
                    "allow" | "allow_always" | "acceptEdits" | "default" | "bypassPermissions"
                );
                let is_always = id == "allow_always";
                // On an ExitPlanMode approval the proceed id IS the mode the user
                // wants to switch into. Record it so the runner can apply it to the
                // session once the tool call finishes (the bridge can't persist it
                // itself — the turn re-saves the session at the end).
                if req.tool_name == "ExitPlanMode" && matches!(id, "acceptEdits" | "default" | "bypassPermissions") {
                    *req.exit_plan_mode.lock().unwrap() = Some(id.to_string());
                }
                (is_allowed, is_always)
            }
            RequestPermissionOutcome::Cancelled => (false, false),
            _ => (false, false),
        },
        Err(e) => {
            warn!(
                error = %e,
                tool = %req.tool_name,
                "permission request failed — denying"
            );
            (false, false)
        }
    };

    if save_always {
        // Update in-memory list immediately so subsequent tool calls this turn
        // are auto-approved without another permission round-trip.
        req.always_allowed.lock().unwrap().push(always_key.clone());
        // Persist to KV store so future turns and sessions also see the decision.
        if let Ok(mut state) = store.load(&req.session_id).await
            && !state.allowed_tools.contains(&always_key)
        {
            state.allowed_tools.push(always_key.clone());
            if let Err(e) = store.save(&req.session_id, &state).await {
                warn!(error = %e, tool = %req.tool_name, "failed to save allowed_tools");
            }
        }
    }

    let _ = req.response_tx.send(allowed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome};
    use tokio::sync::oneshot;
    use trogon_nats::AdvancedMockNatsClient;

    use crate::SessionState;
    use crate::session_store::mock::MemorySessionStore;

    const SESSION: &str = "sess-1";
    const SUBJECT: &str = "acp.session.sess-1.client.session.request_permission";

    fn make_req(tool_name: &str) -> (PermissionReq, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let (started_tx, _started_rx) = oneshot::channel();
        let req = PermissionReq {
            session_id: SESSION.to_string(),
            tool_call_id: "tc-1".to_string(),
            tool_name: tool_name.to_string(),
            tool_input: serde_json::json!({"path": "/tmp/x"}),
            response_tx: tx,
            started_tx,
            always_allowed: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
            exit_plan_mode: std::sync::Arc::new(std::sync::Mutex::new(None)),
        };
        (req, rx)
    }

    fn response(outcome: RequestPermissionOutcome) -> bytes::Bytes {
        serde_json::to_vec(&RequestPermissionResponse::new(outcome))
            .unwrap()
            .into()
    }

    fn selected(id: impl Into<String>) -> bytes::Bytes {
        response(RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
            id.into(),
        )))
    }

    fn cancelled() -> bytes::Bytes {
        response(RequestPermissionOutcome::Cancelled)
    }

    async fn run(tool: &str, nats_response: bytes::Bytes) -> (bool, MemorySessionStore) {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, nats_response);

        let store = MemorySessionStore::new();
        store.save(SESSION, &SessionState::default()).await.unwrap();

        let (req, rx) = make_req(tool);
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;

        (rx.await.unwrap(), store)
    }

    #[tokio::test]
    async fn allow_once_returns_true_no_store_write() {
        let (allowed, store) = run("Bash", selected("allow")).await;
        assert!(allowed);
        assert!(store.load(SESSION).await.unwrap().allowed_tools.is_empty());
    }

    #[tokio::test]
    async fn allow_always_returns_true_and_persists_tool() {
        let (allowed, store) = run("Edit", selected("allow_always")).await;
        assert!(allowed);
        assert!(
            store
                .load(SESSION)
                .await
                .unwrap()
                .allowed_tools
                .contains(&"Edit".to_string())
        );
    }

    #[tokio::test]
    async fn reject_returns_false() {
        let (allowed, _) = run("Write", selected("reject")).await;
        assert!(!allowed);
    }

    #[tokio::test]
    async fn cancelled_returns_false() {
        let (allowed, _) = run("Bash", cancelled()).await;
        assert!(!allowed);
    }

    // ExitPlanMode prompts return mode ids, not "allow". The proceed variants must
    // register as allowed; only "plan" (keep planning) is a deny.
    #[tokio::test]
    async fn exit_plan_mode_accept_edits_returns_true() {
        let (allowed, store) = run("ExitPlanMode", selected("acceptEdits")).await;
        assert!(allowed);
        // A mode-change proceed is not an always-allow of the tool.
        assert!(store.load(SESSION).await.unwrap().allowed_tools.is_empty());
    }

    #[tokio::test]
    async fn exit_plan_mode_default_returns_true() {
        let (allowed, _) = run("ExitPlanMode", selected("default")).await;
        assert!(allowed);
    }

    #[tokio::test]
    async fn exit_plan_mode_bypass_returns_true() {
        let (allowed, _) = run("ExitPlanMode", selected("bypassPermissions")).await;
        assert!(allowed);
    }

    #[tokio::test]
    async fn exit_plan_mode_keep_planning_returns_false() {
        let (allowed, _) = run("ExitPlanMode", selected("plan")).await;
        assert!(!allowed);
    }

    #[tokio::test]
    async fn exit_plan_mode_records_selected_mode_in_cell() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, selected("acceptEdits"));
        let store = MemorySessionStore::new();
        store.save(SESSION, &SessionState::default()).await.unwrap();

        let (req, _rx) = make_req("ExitPlanMode");
        let cell = req.exit_plan_mode.clone();
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;

        assert_eq!(cell.lock().unwrap().as_deref(), Some("acceptEdits"));
    }

    #[tokio::test]
    async fn exit_plan_mode_keep_planning_leaves_cell_empty() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, selected("plan"));
        let store = MemorySessionStore::new();
        store.save(SESSION, &SessionState::default()).await.unwrap();

        let (req, _rx) = make_req("ExitPlanMode");
        let cell = req.exit_plan_mode.clone();
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;

        assert!(cell.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn non_exit_plan_tool_never_writes_mode_cell() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, selected("allow"));
        let store = MemorySessionStore::new();
        store.save(SESSION, &SessionState::default()).await.unwrap();

        let (req, _rx) = make_req("Bash");
        let cell = req.exit_plan_mode.clone();
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;

        assert!(cell.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn allow_always_does_not_duplicate_in_allowed_tools() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, selected("allow_always"));

        let store = MemorySessionStore::new();
        let mut initial = SessionState::default();
        initial.allowed_tools.push("Bash".to_string());
        store.save(SESSION, &initial).await.unwrap();

        let (req, _rx) = make_req("Bash");
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;

        let count = store
            .load(SESSION)
            .await
            .unwrap()
            .allowed_tools
            .iter()
            .filter(|t| *t == "Bash")
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn invalid_session_id_denies() {
        let nats = AdvancedMockNatsClient::new();
        let store = MemorySessionStore::new();
        let (tx, rx) = oneshot::channel();
        let (started_tx, _started_rx) = oneshot::channel();
        let req = PermissionReq {
            session_id: "invalid.session.id".to_string(),
            tool_call_id: "tc-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::Value::Null,
            response_tx: tx,
            started_tx,
            always_allowed: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
            exit_plan_mode: std::sync::Arc::new(std::sync::Mutex::new(None)),
        };
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;
        assert!(!rx.await.unwrap());
    }
}
