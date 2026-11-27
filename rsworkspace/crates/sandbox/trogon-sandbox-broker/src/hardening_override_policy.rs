use std::collections::HashSet;

use trogon_sandbox_contract::ContainerOverrides;
use trogon_sandbox_contract::{LeaseError, TenantId};

/// Which tenants may loosen container hardening for a lease, through
/// `PlatformOptions::container`. Denies every tenant until one is added, so a
/// broker started without configuration never honours an override.
#[derive(Debug, Default, Clone)]
pub struct HardeningOverridePolicy {
    allowed_tenants: HashSet<TenantId>,
}

impl HardeningOverridePolicy {
    pub fn deny_all() -> Self {
        Self::default()
    }

    pub fn allowing(tenants: impl IntoIterator<Item = TenantId>) -> Self {
        Self {
            allowed_tenants: tenants.into_iter().collect(),
        }
    }

    pub fn allows(&self, tenant: &TenantId) -> bool {
        self.allowed_tenants.contains(tenant)
    }

    /// Passes a request that sets no override. Otherwise the tenant must be
    /// allowed; the override itself is already well formed by construction.
    pub fn validate(&self, tenant: &TenantId, requested: &ContainerOverrides) -> Result<(), LeaseError> {
        if requested.is_default() {
            return Ok(());
        }
        if !self.allows(tenant) {
            return Err(LeaseError::InvalidSpec {
                reason: format!("container hardening overrides not permitted for tenant '{tenant}'"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
