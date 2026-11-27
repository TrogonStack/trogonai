use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, PolicyFence, SandboxSpec, TenantId};
use trogon_sandbox_protocol::credential_references;

use crate::{CommittedFenceError, CommittedPolicyStore};

/// Looks up the committed credential policy a lease's credential fetches are fenced against.
///
/// The control plane commits a policy revision per lease before it dispatches the lease. The worker reads that
/// revision when it launches the lease and stamps it on every credential request the lease makes, so the
/// credential broker resolves only references the committed policy names. With `require` set, a lease that needs
/// credentials but has no committed policy is refused instead of falling back to unfenced requests.
#[derive(Clone)]
pub struct CommittedFence {
    store: Arc<dyn CommittedPolicyStore>,
    require: bool,
}

impl CommittedFence {
    pub fn new(store: Arc<dyn CommittedPolicyStore>, require: bool) -> Self {
        Self { store, require }
    }

    /// The fence for the lease, or `None` when the lease needs no credentials or no policy is committed and the
    /// fence is not required.
    pub async fn resolve(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
        spec: &SandboxSpec,
    ) -> Result<Option<PolicyFence>, CommittedFenceError> {
        if credential_references(spec).is_empty() {
            return Ok(None);
        }
        self.resolve_lease(tenant, lease).await
    }

    /// [`Self::resolve`] for a lease already known to need credentials.
    pub async fn resolve_lease(
        &self,
        tenant: &TenantId,
        lease: &LeaseId,
    ) -> Result<Option<PolicyFence>, CommittedFenceError> {
        let committed = match self.store.committed(tenant, lease).await {
            Ok(committed) => committed,
            Err(error) if self.require => return Err(CommittedFenceError::Unavailable(error)),
            Err(error) => {
                tracing::warn!(%error, lease_id = %lease, "committed credential policy unavailable; continuing unfenced");
                None
            }
        };
        match committed {
            Some(policy) => Ok(Some(PolicyFence {
                lease_id: lease.clone(),
                policy_revision: policy.revision,
            })),
            None if self.require => Err(CommittedFenceError::NotCommitted),
            None => Ok(None),
        }
    }
}
