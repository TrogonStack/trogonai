use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use trogon_sandbox_contract::TenantId;

use crate::{EventSender, PreviewState, SandboxBackend, SessionRecord, SessionStore};

/// What a tool call may reach: the session it runs for, the backend, and the socket's event queue.
#[derive(Clone)]
pub(crate) struct ToolContext {
    pub session: SessionRecord,
    pub backend: Arc<dyn SandboxBackend>,
    pub sessions: Arc<dyn SessionStore>,
    pub preview: Option<Arc<PreviewState>>,
    pub events: EventSender,
    /// Cancelled when the socket closes, which stops the dashboard watchers it started.
    pub dashboard_cancel: CancellationToken,
}

impl ToolContext {
    pub(crate) fn tenant(&self) -> TenantId {
        self.session.tenant.clone()
    }

    pub(crate) fn api_key(&self) -> &str {
        &self.session.api_key
    }
}
