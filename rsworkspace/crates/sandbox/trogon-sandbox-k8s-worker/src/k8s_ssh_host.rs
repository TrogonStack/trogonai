use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TerminalRequest, TerminalResponse};
use trogon_sandbox_ssh_gateway::SshTerminalHost;

use crate::K8sTerminals;

/// What the SSH stream bridge needs from the k8s worker: the leases it holds
/// and its terminals.
pub struct K8sSshHost(pub Arc<K8sTerminals>);

#[async_trait]
impl SshTerminalHost for K8sSshHost {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.0.holds(lease_id).await
    }

    async fn terminal(&self, req: &TerminalRequest) -> TerminalResponse {
        self.0.handle_request(req).await
    }
}
