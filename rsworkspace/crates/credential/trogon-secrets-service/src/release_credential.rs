use std::future::Future;

use trogon_secret_store::{SecretStoreCorrelatedGet, SecretStoreError};
use trogon_std::EpochClock;

use super::{
    CredentialRelease, CredentialReleaseError, CredentialStateRead, ReleaseAudit, ReleasePolicySource, ReleaseRequest,
    SealedRelease, Sealer, TenantOwners,
};

/// Releases one credential sealed to a requester, the one operation the
/// release endpoint needs from the release core.
///
/// [`CredentialRelease`] is the implementation the service runs. The endpoint
/// takes this port instead of the core's seven type parameters, so it can be
/// driven by a release that answers whatever a test needs.
pub trait ReleaseCredential: Send + Sync + 'static {
    fn release(
        &self,
        request: &ReleaseRequest,
        requester_xkey: &str,
    ) -> impl Future<Output = Result<SealedRelease, CredentialReleaseError>> + Send;
}

impl<T, P, C, S, A, X, K> ReleaseCredential for CredentialRelease<T, P, C, S, A, X, K>
where
    T: TenantOwners,
    P: ReleasePolicySource,
    C: CredentialStateRead,
    S: SecretStoreCorrelatedGet<Error = SecretStoreError>,
    A: ReleaseAudit,
    X: Sealer,
    K: EpochClock,
{
    fn release(
        &self,
        request: &ReleaseRequest,
        requester_xkey: &str,
    ) -> impl Future<Output = Result<SealedRelease, CredentialReleaseError>> + Send {
        CredentialRelease::release(self, request, requester_xkey)
    }
}
