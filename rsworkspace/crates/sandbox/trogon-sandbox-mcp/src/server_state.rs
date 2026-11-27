use std::sync::Arc;

use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::TenantId;

use crate::policy::Policy;
use crate::tool_error::ToolError;

/// What every tool call needs: the role policy, the tenant, the credential and the broker client.
pub struct ServerState {
    policy: Policy,
    tenant: TenantId,
    api_key: Option<String>,
    client: Option<Arc<SandboxClient>>,
}

impl ServerState {
    pub fn new(policy: Policy, tenant: TenantId, api_key: Option<String>, client: Option<Arc<SandboxClient>>) -> Self {
        Self {
            policy,
            tenant,
            api_key,
            client,
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// Fails unless an API key is configured.
    pub fn require_auth(&self) -> Result<(), ToolError> {
        if self.api_key.is_some() {
            Ok(())
        } else {
            Err(ToolError::Unauthenticated)
        }
    }

    /// The broker client, once authenticated and connected.
    pub fn sandbox_client(&self) -> Result<&SandboxClient, ToolError> {
        self.require_auth()?;
        self.client.as_deref().ok_or(ToolError::NotConnected)
    }
}

#[cfg(test)]
mod tests;
