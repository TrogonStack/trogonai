use std::sync::Arc;

use trogon_sandbox_contract::TenantId;

use super::ServerState;
use crate::policy::Policy;
use crate::testing::ScriptedBus;
use crate::tool_error::ToolError;

fn state(api_key: Option<&str>, connected: bool) -> Result<ServerState, Box<dyn std::error::Error>> {
    let client = connected.then(|| Arc::new(ScriptedBus::silent().into_client()));
    Ok(ServerState::new(
        Policy::default(),
        TenantId::new("acme")?,
        api_key.map(str::to_string),
        client,
    ))
}

#[test]
fn without_a_key_every_call_is_unauthenticated() -> Result<(), Box<dyn std::error::Error>> {
    let state = state(None, true)?;
    assert!(matches!(state.require_auth(), Err(ToolError::Unauthenticated)));
    assert!(matches!(state.sandbox_client(), Err(ToolError::Unauthenticated)));
    Ok(())
}

#[test]
fn a_key_without_a_connection_is_not_connected() -> Result<(), Box<dyn std::error::Error>> {
    let state = state(Some("key"), false)?;
    assert!(state.require_auth().is_ok());
    assert!(matches!(state.sandbox_client(), Err(ToolError::NotConnected)));
    Ok(())
}

#[test]
fn a_key_and_a_connection_yield_the_client() -> Result<(), Box<dyn std::error::Error>> {
    let state = state(Some("key"), true)?;
    assert!(state.sandbox_client().is_ok());
    assert_eq!(state.tenant().as_str(), "acme");
    assert!(state.policy().tools.is_empty());
    Ok(())
}
