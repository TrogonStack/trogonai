//! A [`ClientHandler`] wrapper that rewrites the `session_id` on every client-bound
//! request before delegating to an inner handler.
//!
//! In the IDE multi-runner setup, an external runner runs under its own prefix and its
//! own session id (`runner_sid`), but the IDE client tracks the session under the
//! orchestrator-facing id (`acp_sid`). When the runner emits a client-bound request
//! (`request_permission`, terminal, fs, …) it carries `runner_sid`. This wrapper maps
//! `runner_sid → acp_sid` via a shared table before forwarding to the real IDE
//! connection, so the dialog is associated with the correct session. Ids not present in
//! the table pass through unchanged (e.g. the in-process runner, which already uses the
//! acp id).
//!
//! The NATS reply path is unaffected: the response returns via the NATS `reply-to`
//! subject, which carries no session id, so remapping the request does not break the
//! round-trip.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::Result;
use agent_client_protocol::schema::v1::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest,
    CreateTerminalResponse, ExtNotification, ExtRequest, ExtResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};

use crate::client_handler::ClientHandler;

/// Shared `runner_sid → acp_sid` remap table. Owned by the orchestrator (which populates
/// it when a runner session is created) and shared with the relay.
pub type IdRemap = Arc<Mutex<HashMap<String, String>>>;

/// Wraps an inner [`ClientHandler`] and rewrites `session_id` (`runner_sid → acp_sid`) on
/// every client-bound request before delegating. See module docs.
pub struct RemappingClient<C> {
    inner: Arc<C>,
    id_remap: IdRemap,
}

impl<C> RemappingClient<C> {
    pub fn new(inner: Arc<C>, id_remap: IdRemap) -> Self {
        Self { inner, id_remap }
    }

    /// Map a runner session id to its acp session id. Unmapped ids pass through.
    fn remap(&self, sid: &SessionId) -> SessionId {
        let mapped = self
            .id_remap
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(sid.0.as_ref())
            .cloned();
        match mapped {
            Some(acp_sid) => SessionId::new(acp_sid),
            None => sid.clone(),
        }
    }
}

#[async_trait::async_trait]
impl<C: ClientHandler + Send + Sync> ClientHandler for RemappingClient<C> {
    async fn request_permission(&self, mut args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.request_permission(args).await
    }

    async fn session_notification(&self, mut args: SessionNotification) -> Result<()> {
        args.session_id = self.remap(&args.session_id);
        self.inner.session_notification(args).await
    }

    async fn read_text_file(&self, mut args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.read_text_file(args).await
    }

    async fn write_text_file(&self, mut args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.write_text_file(args).await
    }

    async fn create_terminal(&self, mut args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.create_terminal(args).await
    }

    async fn terminal_output(&self, mut args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.terminal_output(args).await
    }

    async fn release_terminal(&self, mut args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.release_terminal(args).await
    }

    async fn wait_for_terminal_exit(&self, mut args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.wait_for_terminal_exit(args).await
    }

    async fn kill_terminal(&self, mut args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        args.session_id = self.remap(&args.session_id);
        self.inner.kill_terminal(args).await
    }

    // The following carry no session id; forward transparently to the inner handler.
    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        self.inner.ext_method(args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.inner.ext_notification(args).await
    }

    async fn elicitation_create(&self, args: CreateElicitationRequest) -> Result<CreateElicitationResponse> {
        self.inner.elicitation_create(args).await
    }

    async fn elicitation_complete(&self, args: CompleteElicitationNotification) -> Result<()> {
        self.inner.elicitation_complete(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{RequestPermissionOutcome, ToolCallUpdate, ToolCallUpdateFields};

    /// Inner handler that records the `session_id` it actually received.
    struct CapturingClient {
        seen: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ClientHandler for CapturingClient {
        async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
            *self.seen.lock().unwrap() = Some(args.session_id.0.to_string());
            Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
        }

        async fn session_notification(&self, _: SessionNotification) -> Result<()> {
            Ok(())
        }
    }

    fn perm_req(session_id: &str) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            SessionId::new(session_id),
            ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
            vec![],
        )
    }

    #[tokio::test]
    async fn mapped_session_id_is_rewritten_before_delegating() {
        let seen = Arc::new(Mutex::new(None));
        let inner = Arc::new(CapturingClient { seen: seen.clone() });
        let map: IdRemap = Arc::new(Mutex::new(HashMap::new()));
        map.lock().unwrap().insert("runner-sid".to_string(), "acp-sid".to_string());

        let client = RemappingClient::new(inner, map);
        client.request_permission(perm_req("runner-sid")).await.unwrap();

        assert_eq!(
            seen.lock().unwrap().as_deref(),
            Some("acp-sid"),
            "inner client must receive the remapped acp session id"
        );
    }

    #[tokio::test]
    async fn unmapped_session_id_passes_through_unchanged() {
        let seen = Arc::new(Mutex::new(None));
        let inner = Arc::new(CapturingClient { seen: seen.clone() });
        let map: IdRemap = Arc::new(Mutex::new(HashMap::new()));

        let client = RemappingClient::new(inner, map);
        client.request_permission(perm_req("unmapped-sid")).await.unwrap();

        assert_eq!(
            seen.lock().unwrap().as_deref(),
            Some("unmapped-sid"),
            "ids not in the table must pass through unchanged"
        );
    }
}
