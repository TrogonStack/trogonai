use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_auth::ApiKeyStore;

use crate::{PreviewState, SandboxBackend, SessionStore};

/// What every request handler shares.
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<dyn SessionStore>,
    pub api_keys: Arc<dyn ApiKeyStore>,
    pub backend: Arc<dyn SandboxBackend>,
    pub session_ttl: Duration,
    /// `None` disables preview URLs and the preview proxy route.
    pub preview: Option<Arc<PreviewState>>,
}
