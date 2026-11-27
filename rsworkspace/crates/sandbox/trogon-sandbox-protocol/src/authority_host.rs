use async_trait::async_trait;

use crate::{AuthorityHostError, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

/// What the authority guard acts on. Mutations must compare the exact assignment under the same gate that changes
/// it. A manual pause invalidates any guard-owned thaw transition.
#[async_trait]
pub trait AuthorityHost: Send + Sync {
    /// The leases the worker holds.
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError>;

    /// Pauses `lease`. Hosts that cannot pause report [`FreezeOutcome::Unsupported`].
    async fn freeze(&self, _lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        Ok(FreezeOutcome::Unsupported)
    }

    /// Resumes a lease the guard froze.
    async fn thaw(&self, _lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        Ok(ThawOutcome::NoLongerOwned)
    }

    /// Success requires exact backend cleanup or absence proof. Map absence alone is not proof that a failed
    /// cleanup settled.
    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError>;
}
