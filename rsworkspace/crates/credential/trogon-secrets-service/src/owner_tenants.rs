use trogon_credential_domain::CredentialOwnerId;

use super::TenantAccount;

/// Which tenant holds each credential owner, the reverse of [`TenantOwners`].
///
/// An invalidation notice goes out on the subject that names the one account
/// holding the credential's owner (ADR#0095 Decision 5). It is a port of its
/// own because the release core only ever asks the forward question.
///
/// [`TenantOwners`]: super::TenantOwners
pub trait OwnerTenants: Send + Sync + 'static {
    /// The tenant holding `owner`, or `None` when no tenant does.
    fn holder(&self, owner: &CredentialOwnerId) -> Option<TenantAccount>;
}
