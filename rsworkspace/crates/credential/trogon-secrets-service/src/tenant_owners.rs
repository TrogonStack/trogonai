use trogon_credential_domain::CredentialOwnerId;

use super::{OwnerHolding, TenantAccount};

/// Which credential owners each tenant holds.
///
/// The release core asks before anything else about the credential, so a
/// tenant can never reach another tenant's credentials by naming them.
pub trait TenantOwners: Send + Sync + 'static {
    fn holding(&self, tenant: &TenantAccount, owner: &CredentialOwnerId) -> OwnerHolding;
}
