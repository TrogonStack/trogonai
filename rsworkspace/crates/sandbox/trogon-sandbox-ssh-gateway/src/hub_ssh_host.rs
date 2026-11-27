use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TerminalRequest, TerminalResponse};

use crate::{SshTerminalHost, TerminalHub};

/// What the SSH stream bridge needs from a worker: the leases it holds and its terminals.
pub struct HubSshHost(pub Arc<TerminalHub>);

#[async_trait]
impl SshTerminalHost for HubSshHost {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.0.holds(lease_id).await
    }

    async fn terminal(&self, request: &TerminalRequest) -> TerminalResponse {
        self.0.handle(request).await
    }
}
