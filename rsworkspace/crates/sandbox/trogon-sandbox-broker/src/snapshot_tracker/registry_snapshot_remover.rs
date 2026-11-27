use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{Digest, TenantId};

use super::remove_accounted_snapshot::remove_accounted_snapshot;
use super::snapshot_removal::SnapshotRemoval;
use super::snapshot_removal_error::SnapshotRemovalError;
use super::snapshot_remover::SnapshotRemover;
use crate::snapshot_live::LiveSnapshotStore;

/// [`SnapshotRemover`] backed by the live snapshot set (for ownership and
/// size) and the OCI registry (for the artifact itself).
pub struct RegistrySnapshotRemover {
    live: Arc<dyn LiveSnapshotStore>,
    oci: Arc<trogon_sandbox_oci::OciClient>,
    accounting: Option<Arc<crate::snapshot_accounting::SnapshotAccountingStore>>,
}

impl RegistrySnapshotRemover {
    pub fn new(
        live: Arc<dyn LiveSnapshotStore>,
        oci: Arc<trogon_sandbox_oci::OciClient>,
        _registry: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            live,
            oci,
            accounting: None,
        })
    }

    pub fn with_accounting(
        live: Arc<dyn LiveSnapshotStore>,
        oci: Arc<trogon_sandbox_oci::OciClient>,
        _registry: String,
        accounting: Arc<crate::snapshot_accounting::SnapshotAccountingStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            live,
            oci,
            accounting: Some(accounting),
        })
    }
}

#[async_trait]
impl SnapshotRemover for RegistrySnapshotRemover {
    async fn remove(&self, tenant: &TenantId, digest: &Digest) -> Result<SnapshotRemoval, SnapshotRemovalError> {
        if let Some(accounting) = &self.accounting {
            return Ok(
                match remove_accounted_snapshot(accounting, self.live.as_ref(), self.oci.as_ref(), tenant, digest)
                    .await?
                {
                    Some(claim) => SnapshotRemoval::Removed {
                        bytes: claim.entry.bytes,
                        generation: Some(claim.generation),
                    },
                    None => SnapshotRemoval::NotFound,
                },
            );
        }
        let Some(entry) = self.live.get_owned(tenant, digest).await? else {
            return Ok(SnapshotRemoval::NotFound);
        };
        if entry.tenant != *tenant {
            return Ok(SnapshotRemoval::NotFound);
        }
        Err(SnapshotRemovalError::ClaimRequired)
    }
}
