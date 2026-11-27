use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::subjects::provider_terminal;
use trogon_sandbox_ssh_gateway::run_terminal_handler as serve_terminals;
use trogon_sandbox_ssh_gateway::{TerminalHub, TunnelError};

/// Serves the KubeVirt broadcast terminal subject. Every KubeVirt worker receives each request; only the one holding
/// the lease acts and replies.
pub async fn run_terminal_handler(client: Client, hub: Arc<TerminalHub>) -> Result<(), TunnelError> {
    let subject = provider_terminal(ProviderKind::KubeVirt, &LeaseId::new(">"));
    serve_terminals(client, hub, &subject).await
}
