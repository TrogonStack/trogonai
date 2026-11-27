use async_trait::async_trait;

use crate::{Authorizer, AuthzError, SandboxAuthzRequest};

/// An [`Authorizer`] that allows every request, for deployments without an external authorizer.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopAuthorizer;

#[async_trait]
impl Authorizer for NoopAuthorizer {
    async fn authorize_sandbox_lease(&self, _request: &SandboxAuthzRequest) -> Result<(), AuthzError> {
        Ok(())
    }
}
