use std::collections::{BTreeMap, BTreeSet};

use trogon_credential_domain::CredentialOwnerId;

use super::{OwnerHolding, TenantAccount, TenantOwners};

/// Tenant owners loaded once at startup.
#[derive(Clone, Debug, Default)]
pub struct StaticTenantOwners {
    owners: BTreeMap<TenantAccount, BTreeSet<CredentialOwnerId>>,
}

impl StaticTenantOwners {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `tenant` holds `owner`.
    #[must_use]
    pub fn with_owner(mut self, tenant: TenantAccount, owner: CredentialOwnerId) -> Self {
        self.owners.entry(tenant).or_default().insert(owner);
        self
    }
}

impl TenantOwners for StaticTenantOwners {
    fn holding(&self, tenant: &TenantAccount, owner: &CredentialOwnerId) -> OwnerHolding {
        match self.owners.get(tenant) {
            None => OwnerHolding::UnknownTenant,
            Some(owners) if owners.contains(owner) => OwnerHolding::Held,
            Some(_) => OwnerHolding::NotHeld,
        }
    }
}

#[cfg(test)]
mod tests;
