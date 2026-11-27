//! Commits each lease's credential policy for the credential broker's fence.
//!
//! The policy is the set of credential references the lease's spec names. It
//! is committed before a lease is dispatched (revision 1), again whenever a
//! network policy update changes the set (a higher revision), and deleted when
//! the lease is released. The credential broker resolves only references the
//! committed policy names at the revision a caller presents.

use std::sync::Arc;

use tracing::warn;
use trogon_sandbox_contract::{LeaseError, LeaseId, SandboxSpec, TenantId};
use trogon_sandbox_credential_broker::CommittedPolicyWriter;
use trogon_sandbox_protocol::credential_references;

use super::BrokerCore;

impl BrokerCore {
    /// Commits credential policies through `writer`. Without it nothing is
    /// committed, which is how the broker ran before the fence existed.
    pub fn with_committed_policy_writer(mut self, writer: Arc<dyn CommittedPolicyWriter>) -> Self {
        self.committed_policies = Some(writer);
        self
    }

    /// [`Self::with_committed_policy_writer`] for a writer that may be absent.
    pub fn with_committed_policy_writer_if(mut self, writer: Option<Arc<dyn CommittedPolicyWriter>>) -> Self {
        if writer.is_some() {
            self.committed_policies = writer;
        }
        self
    }

    /// Commits the references `spec` names for the lease and returns the
    /// revision, or `None` when no writer is configured. A store failure
    /// denies the request rather than leaving a lease with no policy behind.
    pub(crate) async fn commit_credential_policy(
        &self,
        tenant: &TenantId,
        lease_id: &LeaseId,
        spec: &SandboxSpec,
    ) -> Result<Option<u64>, LeaseError> {
        let Some(writer) = &self.committed_policies else {
            return Ok(None);
        };
        writer
            .commit(tenant, lease_id, credential_references(spec))
            .await
            .map(Some)
            .map_err(|error| LeaseError::Backend {
                message: format!("committing the credential policy failed: {error}"),
            })
    }

    /// Deletes the lease's committed policy. Best effort: a lease that is
    /// gone resolves nothing either way, and a failure here is logged.
    pub(crate) async fn revoke_credential_policy(&self, tenant: &TenantId, lease_id: &LeaseId) {
        let Some(writer) = &self.committed_policies else {
            return;
        };
        if let Err(error) = writer.revoke(tenant, lease_id).await {
            warn!(%error, lease_id = %lease_id, "failed to delete the committed credential policy");
        }
    }
}
