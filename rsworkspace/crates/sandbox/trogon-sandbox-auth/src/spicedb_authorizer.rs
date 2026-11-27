use async_trait::async_trait;

use crate::{
    Authorizer, AuthzError, SandboxAuthzRequest, SpiceDbClient, SpiceDbConnectError, SpiceDbEndpoint, SpiceDbToken,
    TenantResourceType,
};

/// An [`Authorizer`] that asks the permission service whether the service account holds the
/// sandbox lease permission on the tenant.
#[derive(Debug, Clone)]
pub struct SpiceDbAuthorizer {
    client: SpiceDbClient,
}

impl SpiceDbAuthorizer {
    pub async fn connect(endpoint: &SpiceDbEndpoint, token: &SpiceDbToken) -> Result<Self, SpiceDbConnectError> {
        Ok(Self::new(SpiceDbClient::connect(endpoint, token).await?))
    }

    pub async fn connect_with_resource_type(
        endpoint: &SpiceDbEndpoint,
        token: &SpiceDbToken,
        tenant_resource_type: TenantResourceType,
    ) -> Result<Self, SpiceDbConnectError> {
        Ok(Self::new(
            SpiceDbClient::connect_with_resource_type(endpoint, token, tenant_resource_type).await?,
        ))
    }

    pub fn new(client: SpiceDbClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Authorizer for SpiceDbAuthorizer {
    async fn authorize_sandbox_lease(&self, request: &SandboxAuthzRequest) -> Result<(), AuthzError> {
        self.client
            .check_sandbox_lease(&request.tenant, &request.principal)
            .await
    }
}
