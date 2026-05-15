//! Bridges the in-process `PermissionReq` channel to the ACP client via NATS.
//!
//! When `ChannelPermissionChecker` fires, it sends a `PermissionReq` over an mpsc
//! channel. The task in `main.rs` drains that channel and calls
//! `handle_permission_request_nats` for each request, which forwards it to the
//! ACP client via `NatsClientProxy::request_permission` (NATS request-reply).
//!
//! This is the NATS equivalent of `handle_permission_request` in the stdio runner
//! (`trogon-acp/src/main.rs`). The logic is identical; only the transport differs.

use std::time::Duration;

use agent_client_protocol::{
    Client as AcpClient, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, ToolCallUpdate, ToolCallUpdateFields,
};
use acp_nats::{acp_prefix::AcpPrefix, client_proxy::NatsClientProxy, session_id::AcpSessionId};
use trogon_nats::{FlushClient, PublishClient, RequestClient};
use tracing::warn;

use trogon_runner_tools::{PermissionReq, SessionStore};

/// Forward a single `PermissionReq` to the ACP client and send the allow/deny
/// decision back on the embedded oneshot channel.
///
/// Options presented to the user:
/// - **Always Allow** — allow this turn and persist the tool to `allowed_tools`
///   so future calls within the session are auto-approved.
/// - **Allow** — allow this turn only.
/// - **Reject** — deny; the agent receives `false` and reports permission denied.
///
/// On network error or client cancellation the request is denied.
pub async fn handle_permission_request_nats<S, N>(
    req: PermissionReq,
    nats: N,
    prefix: AcpPrefix,
    store: &S,
) where
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

    let proxy = NatsClientProxy::new(nats, session_id, prefix, Duration::from_secs(30));

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

    let outcome = proxy.request_permission(perm_req).await;

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
            warn!(
                error = %e,
                tool = %req.tool_name,
                "permission request failed — denying"
            );
            (false, false)
        }
    };

    if save_always
        && let Ok(mut state) = store.load(&req.session_id).await
        && !state.allowed_tools.contains(&req.tool_name)
    {
        state.allowed_tools.push(req.tool_name.clone());
        if let Err(e) = store.save(&req.session_id, &state).await {
            warn!(error = %e, tool = %req.tool_name, "failed to save allowed_tools");
        }
    }

    let _ = req.response_tx.send(allowed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        RequestPermissionOutcome, RequestPermissionResponse, SelectedPermissionOutcome,
    };
    use tokio::sync::oneshot;
    use trogon_nats::AdvancedMockNatsClient;

    use trogon_runner_tools::session_store::mock::MemorySessionStore;
    use trogon_runner_tools::SessionState;

    const SESSION: &str = "sess-1";
    const SUBJECT: &str = "acp.session.sess-1.client.session.request_permission";

    fn make_req(tool_name: &str) -> (PermissionReq, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let req = PermissionReq {
            session_id: SESSION.to_string(),
            tool_call_id: "tc-1".to_string(),
            tool_name: tool_name.to_string(),
            tool_input: serde_json::json!({"path": "/tmp/x"}),
            response_tx: tx,
        };
        (req, rx)
    }

    fn response(outcome: RequestPermissionOutcome) -> bytes::Bytes {
        serde_json::to_vec(&RequestPermissionResponse::new(outcome))
            .unwrap()
            .into()
    }

    fn selected(id: impl Into<String>) -> bytes::Bytes {
        response(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(id.into()),
        ))
    }

    fn cancelled() -> bytes::Bytes {
        response(RequestPermissionOutcome::Cancelled)
    }

    async fn run(tool: &str, nats_response: bytes::Bytes) -> (bool, MemorySessionStore) {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, nats_response);

        let store = MemorySessionStore::new();
        store
            .save(SESSION, &SessionState::default())
            .await
            .unwrap();

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
        assert!(store
            .load(SESSION)
            .await
            .unwrap()
            .allowed_tools
            .contains(&"Edit".to_string()));
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
        let req = PermissionReq {
            session_id: "invalid.session.id".to_string(), // dots are not allowed
            tool_call_id: "tc-1".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::Value::Null,
            response_tx: tx,
        };
        handle_permission_request_nats(req, nats, AcpPrefix::new("acp").unwrap(), &store).await;
        assert!(!rx.await.unwrap());
    }
}

