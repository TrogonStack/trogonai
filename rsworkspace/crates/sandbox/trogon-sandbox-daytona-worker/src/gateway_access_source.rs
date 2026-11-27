use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

use crate::{GatewayAccess, GatewayAccessError};

/// What the SSH client needs from Daytona. [`crate::DaytonaGatewayAccess`] implements it over the Daytona API; tests
/// point it at an in-process server.
#[async_trait]
pub trait GatewayAccessSource: Send + Sync {
    /// Whether this worker holds `lease_id`.
    async fn holds(&self, lease_id: &LeaseId) -> bool;

    /// Mints a fresh access token and the gateway endpoint for the lease.
    async fn mint(&self, lease_id: &LeaseId) -> Result<GatewayAccess, GatewayAccessError>;
}
