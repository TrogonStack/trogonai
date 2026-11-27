use async_trait::async_trait;

use crate::{AuthzError, SandboxAuthzRequest};

/// Decides whether a service account may lease sandboxes for a tenant.
#[async_trait]
pub trait Authorizer: Send + Sync {
    /// `Ok(())` when allowed; [`AuthzError::Denied`] when the permission is absent.
    async fn authorize_sandbox_lease(&self, request: &SandboxAuthzRequest) -> Result<(), AuthzError>;
}
