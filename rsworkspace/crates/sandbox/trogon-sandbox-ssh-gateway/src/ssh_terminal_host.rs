use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TerminalRequest, TerminalResponse};

/// What a worker provides so the bridge can serve SSH on it.
#[async_trait]
pub trait SshTerminalHost: Send + Sync {
    /// Whether this worker currently holds `lease_id`.
    async fn holds(&self, lease_id: &LeaseId) -> bool;

    /// Answers one terminal request for a lease this worker holds. The terminal's output must be published on its
    /// terminal output subject.
    async fn terminal(&self, request: &TerminalRequest) -> TerminalResponse;
}
