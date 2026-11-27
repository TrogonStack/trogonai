use std::sync::Arc;

use trogon_sandbox_contract::TenantId;

use super::ScriptedBus;
use crate::policy::Policy;
use crate::server::SandboxMcpServer;
use crate::server_state::ServerState;

/// A server for tenant `acme` with an API key, answering from `bus` and enforcing `policy`.
pub(crate) fn test_server(bus: ScriptedBus, policy: Policy) -> Result<SandboxMcpServer, Box<dyn std::error::Error>> {
    let client = Arc::new(bus.into_client());
    let state = ServerState::new(policy, TenantId::new("acme")?, Some("key".to_string()), Some(client));
    Ok(SandboxMcpServer::new(Arc::new(state)))
}
