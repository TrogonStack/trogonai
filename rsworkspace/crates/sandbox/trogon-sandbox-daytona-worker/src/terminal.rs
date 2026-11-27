//! Interactive terminals backed by an SSH channel into the sandbox.
//!
//! The hub is the one the SSH-backed workers share, in [`trogon_sandbox_ssh_gateway::TerminalHub`]; this module names
//! it for Daytona and fixes the broker subject. [`crate::RusshGatewayShell`] is the Daytona
//! [`trogon_sandbox_ssh_gateway::GuestShell`].

use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_ssh_gateway::TunnelError;
pub use trogon_sandbox_ssh_gateway::{
    DEFAULT_TERMINAL_IDLE_TIMEOUT, HubSshHost as DaytonaSshHost, TerminalHub as DaytonaTerminals,
};

use crate::constants::TERMINAL_SUBJECT;

/// Serves [`TERMINAL_SUBJECT`]. Every Daytona worker receives each request; only the one holding the lease acts and
/// replies.
pub async fn run_terminal_handler(client: Client, terminals: Arc<DaytonaTerminals>) -> Result<(), TunnelError> {
    trogon_sandbox_ssh_gateway::run_terminal_handler(client, terminals, TERMINAL_SUBJECT).await
}
