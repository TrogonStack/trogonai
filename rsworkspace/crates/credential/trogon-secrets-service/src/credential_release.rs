use trogon_credential_aggregate::releasable_version;
use trogon_secret_store::{SecretMaterial, SecretStoreError, SecretStoreGet};
use trogon_std::EpochClock;

use super::constants::RELEASE_TTL_BOUND;
use super::refusal::Refusal;
use super::release_fault_error::ReleaseFaultError;
use super::{
    CredentialReleaseError, CredentialStateRead, DenialReason, DenialRecord, OwnerHolding, ReleaseAudit,
    ReleasePolicySource, ReleaseRecord, ReleaseRequest, RequesterXkey, SealedRelease, Sealer, TenantOwners,
};

/// Decides whether a runtime service may receive a credential, and seals it to
/// the requester when it may.
///
/// Each check runs in a fixed order and stops at the first refusal: the
/// requester xkey, the tenant's hold on the credential's owner, the release
/// policy, the credential's state, and the stored value. A release is audited
/// with a durable acknowledgement before it is sealed, so no value leaves
/// without a record of it.
pub struct CredentialRelease<T, P, C, S, A, X, K> {
    tenant_owners: T,
    policies: P,
    states: C,
    store: S,
    audit: A,
    sealer: X,
    clock: K,
}

impl<T, P, C, S, A, X, K> CredentialRelease<T, P, C, S, A, X, K>
where
    T: TenantOwners,
    P: ReleasePolicySource,
    C: CredentialStateRead,
    S: SecretStoreGet<Error = SecretStoreError>,
    A: ReleaseAudit,
    X: Sealer,
    K: EpochClock,
{
    pub fn new(tenant_owners: T, policies: P, states: C, store: S, audit: A, sealer: X, clock: K) -> Self {
        Self {
            tenant_owners,
            policies,
            states,
            store,
            audit,
            sealer,
            clock,
        }
    }

    /// Releases the requested credential sealed to `requester_xkey`.
    pub async fn release(
        &self,
        request: &ReleaseRequest,
        requester_xkey: &str,
    ) -> Result<SealedRelease, CredentialReleaseError> {
        match self.decide(request, requester_xkey).await {
            Ok(release) => Ok(release),
            Err(Refusal::Denied(reason)) => Err(self.deny(request, reason).await),
            Err(Refusal::Faulted(fault)) => Err(fail(request, &fault)),
        }
    }

    async fn decide(&self, request: &ReleaseRequest, requester_xkey: &str) -> Result<SealedRelease, Refusal> {
        let recipient = RequesterXkey::parse(requester_xkey).map_err(|_| DenialReason::RequesterXkeyInvalid)?;
        match self
            .tenant_owners
            .holding(request.tenant(), request.credential().owner_id())
        {
            OwnerHolding::Held => {}
            OwnerHolding::UnknownTenant => return Err(DenialReason::UnknownTenant.into()),
            OwnerHolding::NotHeld => return Err(DenialReason::OwnerNotHeld.into()),
        }
        let policy = self
            .policies
            .policy(request.credential())
            .ok_or(DenialReason::NoReleasePolicy)?;
        policy
            .permits(&request.policy_request())
            .map_err(DenialReason::PolicyRefused)?;
        let state = self
            .states
            .read(request.credential())
            .await
            .map_err(ReleaseFaultError::StateRead)?
            .ok_or(DenialReason::UnknownCredential)?;
        let credential = releasable_version(&state)
            .map_err(ReleaseFaultError::StateUndecodable)?
            .ok_or(DenialReason::NotReleasable)?;
        if credential.key() != *request.credential() {
            return Err(ReleaseFaultError::OtherCredential {
                found: credential.key(),
            }
            .into());
        }
        let value = match self.store.get(&credential).await? {
            SecretMaterial::Plaintext(value) => value,
            SecretMaterial::Verifier(_) => return Err(DenialReason::VerifierOnly.into()),
        };
        let valid_until = self.clock.system_time() + policy.effective_cache_ttl(RELEASE_TTL_BOUND);
        let record = ReleaseRecord::new(request.clone(), credential.version(), recipient.clone(), valid_until);
        self.audit
            .record_release(&record)
            .await
            .map_err(ReleaseFaultError::AuditUnavailable)?;
        let sealed = self
            .sealer
            .seal(&value, &recipient)
            .map_err(ReleaseFaultError::SealFailed)?;
        tracing::info!(version = credential.version().get(), tenant = %request.tenant(), credential = %request.credential(), runtime_service = %request.runtime_service(), destination = %request.destination(), location = %request.location(), correlation_id = %request.correlation_id(), "credential released");
        Ok(SealedRelease::new(sealed, valid_until))
    }

    async fn deny(&self, request: &ReleaseRequest, reason: DenialReason) -> CredentialReleaseError {
        let failure = reason.failure();
        tracing::warn!(reason = reason.code(), detail = %reason, tenant = %request.tenant(), credential = %request.credential(), runtime_service = %request.runtime_service(), destination = %request.destination(), location = %request.location(), correlation_id = %request.correlation_id(), "credential release denied");
        let record = DenialRecord::new(request.clone(), reason);
        if let Err(error) = self.audit.record_denial(&record).await {
            tracing::error!(error = %error, tenant = %request.tenant(), credential = %request.credential(), runtime_service = %request.runtime_service(), destination = %request.destination(), location = %request.location(), correlation_id = %request.correlation_id(), "credential release denial was not audited; the denial stands");
        }
        failure
    }
}

fn fail(request: &ReleaseRequest, fault: &ReleaseFaultError) -> CredentialReleaseError {
    tracing::error!(fault = fault.code(), error = %fault, tenant = %request.tenant(), credential = %request.credential(), runtime_service = %request.runtime_service(), destination = %request.destination(), location = %request.location(), correlation_id = %request.correlation_id(), "credential release failed");
    fault.failure()
}

#[cfg(test)]
mod tests;
