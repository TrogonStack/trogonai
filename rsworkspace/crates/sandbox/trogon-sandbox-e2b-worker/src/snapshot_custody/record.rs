use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::custody_error::SnapshotCustodyError;
use super::phase::CustodyPhase;
use crate::snapshots::{SnapshotTemplateId, is_opaque_id, validate_reference_identity};

/// Who owns a snapshot, what it is, and when it may next be cleaned up.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustodyRecord {
    pub(crate) version: u32,
    pub(crate) tenant: TenantId,
    pub(crate) reference: NativeSnapshotRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) template_id: Option<SnapshotTemplateId>,
    pub(crate) phase: CustodyPhase,
    pub(crate) attempts: u32,
    pub(crate) next_attempt_unix_ms: u64,
    pub(crate) claim_until_unix_ms: u64,
}

impl CustodyRecord {
    /// A record for a capture that has started but has not yet produced a template.
    pub(crate) fn capturing(tenant: TenantId, reference: NativeSnapshotRef, until: u64) -> Self {
        Self {
            version: 1,
            tenant,
            reference,
            template_id: None,
            phase: CustodyPhase::Capturing,
            attempts: 0,
            next_attempt_unix_ms: 0,
            claim_until_unix_ms: until,
        }
    }

    pub(super) fn validate(&self) -> Result<(), SnapshotCustodyError> {
        if self.version != 1 {
            return Err(SnapshotCustodyError::InvalidVersion);
        }
        validate_reference_identity(&self.reference)?;
        if !is_opaque_id(self.reference.id.as_str()) {
            return Err(SnapshotCustodyError::InvalidReferenceId);
        }
        if (self.phase == CustodyPhase::Capturing) != self.template_id.is_none() {
            return Err(SnapshotCustodyError::IdentityMissing);
        }
        Ok(())
    }

    pub(super) fn key(&self) -> String {
        tenant_scoped_kv_key(&self.tenant, self.reference.id.as_str())
    }
}
