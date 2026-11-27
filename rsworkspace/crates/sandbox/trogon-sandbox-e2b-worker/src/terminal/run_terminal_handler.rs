use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_ssh_gateway::TunnelError;

use super::E2bTerminals;
use super::constants::TERMINAL_SUBJECT;

/// Serves [`TERMINAL_SUBJECT`]. Every E2B worker receives each request; only the one holding the lease acts and
/// replies.
pub async fn run_terminal_handler(client: Client, terminals: Arc<E2bTerminals>) -> Result<(), TunnelError> {
    trogon_sandbox_ssh_gateway::run_terminal_handler(client, terminals, TERMINAL_SUBJECT).await
}
