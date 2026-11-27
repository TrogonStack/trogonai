use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, SshAccessResponse};

use crate::{DaytonaWorkerProvider, GatewayAccess, GatewayAccessError, GatewayAccessSource};

/// [`GatewayAccessSource`] over the Daytona API.
pub struct DaytonaGatewayAccess {
    provider: Arc<DaytonaWorkerProvider>,
}

impl DaytonaGatewayAccess {
    pub fn new(provider: Arc<DaytonaWorkerProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl GatewayAccessSource for DaytonaGatewayAccess {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.provider.sandbox_for_lease(lease_id).await.is_some()
    }

    async fn mint(&self, lease_id: &LeaseId) -> Result<GatewayAccess, GatewayAccessError> {
        let sandbox = self
            .provider
            .sandbox_for_lease(lease_id)
            .await
            .ok_or_else(|| GatewayAccessError::UnknownLease(lease_id.clone()))?;
        match self.provider.ssh_access_for_sandbox(&sandbox.id, None).await {
            SshAccessResponse::Available(access) => Ok(GatewayAccess {
                host: access.host,
                port: access.port,
                token: access.user,
                expires_at: access.expires_at,
            }),
            SshAccessResponse::Unsupported => Err(GatewayAccessError::Unsupported),
            SshAccessResponse::Denied(error) => Err(GatewayAccessError::Refused(format!("{error:?}"))),
        }
    }
}
