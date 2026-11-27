use trogon_sandbox_contract::{
    AccountLimits, GetAccountLimitsRequest, GetAccountLimitsResponse, GetLeaseUsageRequest, GetLeaseUsageResponse,
    GetTenantUsageRequest, GetTenantUsageResponse, LeaseId, LeaseUsage, TenantId, TenantUsage,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::SLOW_VERB_TIMEOUT;
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Usage counters for `lease_id` (see [`LeaseUsage`]): commands run, bytes
    /// moved, running and paused time, and resource gauges when the provider
    /// reports them. A released lease returns its final figures until the
    /// broker ages them out; an unknown or foreign lease is `LeaseNotFound`.
    pub async fn usage(&self, tenant: TenantId, lease_id: &LeaseId) -> Result<LeaseUsage, ClientError> {
        let request = GetLeaseUsageRequest {
            tenant,
            lease_id: lease_id.clone(),
        };
        match self
            .send(&subjects::lease_usage(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            GetLeaseUsageResponse::Usage { usage } => Ok(usage),
            GetLeaseUsageResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Counters summed over `tenant`'s live leases.
    pub async fn tenant_usage(&self, tenant: TenantId) -> Result<TenantUsage, ClientError> {
        let request = GetTenantUsageRequest { tenant };
        match self
            .send(subjects::TENANT_USAGE_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            GetTenantUsageResponse::Usage { usage } => Ok(usage),
            GetTenantUsageResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// The tenant's broker-side limits next to what it has consumed: live
    /// leases, monthly CPU and wall budgets, per-request caps and any per-tag
    /// cap with its current counts.
    pub async fn limits(&self, tenant: TenantId) -> Result<AccountLimits, ClientError> {
        let request = GetAccountLimitsRequest { tenant };
        match self
            .send(subjects::TENANT_LIMITS_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            GetAccountLimitsResponse::Limits { limits } => Ok(limits),
            GetAccountLimitsResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }
}
