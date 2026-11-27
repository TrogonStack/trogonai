use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};

use super::binding::SnapshotBinding;
use super::captured_execution::CapturedExecution;
use super::constants::MAX_EXECUTIONS;
use super::logical_name::validate_logical_name;
use super::manifest_error::SnapshotManifestError;
use super::opaque_id::is_opaque_id;
use super::reference::{validate_reference, validate_reference_identity};

/// What a captured snapshot records about its owner, its commands, and the build it restores from.
///
/// Versions 1 and 2 are historical and cannot be restored; version 3 carries the immutable binding.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotManifest {
    pub(crate) version: u32,
    pub(crate) tenant: TenantId,
    pub(crate) reference: NativeSnapshotRef,
    pub(crate) executions: Vec<CapturedExecution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) binding: Option<SnapshotBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) logical_name: Option<String>,
}

impl SnapshotManifest {
    /// Validates against `tenant` and `reference`, and requires both references to be unexpired.
    pub(crate) fn validate(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<(), SnapshotManifestError> {
        validate_reference(reference)?;
        validate_reference(&self.reference)?;
        self.validate_identity(tenant, reference)
    }

    /// Validates against `tenant` and `reference`; expired references still pass so they can be deleted.
    pub(crate) fn validate_identity(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<(), SnapshotManifestError> {
        validate_reference_identity(reference)?;
        validate_reference_identity(&self.reference)?;
        if !matches!(self.version, 1..=3) || &self.tenant != tenant {
            return Err(SnapshotManifestError::OwnerOrVersionMismatch);
        }
        if self.reference.provider != reference.provider || self.reference.id != reference.id {
            return Err(SnapshotManifestError::IdentityMismatch);
        }
        if self.executions.len() > MAX_EXECUTIONS {
            return Err(SnapshotManifestError::TooManyExecutions);
        }
        self.validate_capture_binding()?;
        let mut seen = BTreeSet::new();
        for execution in &self.executions {
            if execution.deadline.is_some() != matches!(self.version, 2 | 3) {
                return Err(SnapshotManifestError::DeadlineVersionMismatch);
            }
            if let Some(deadline) = &execution.deadline {
                deadline.validate()?;
            }
            if !is_opaque_id(&execution.execution_id) {
                return Err(SnapshotManifestError::InvalidExecutionId);
            }
            if !seen.insert(&execution.execution_id) {
                return Err(SnapshotManifestError::DuplicateExecutionId);
            }
            if !execution.identity.is_real() {
                return Err(SnapshotManifestError::InvalidProcessIdentity);
            }
            usize::try_from(execution.max_output_bytes)?;
        }
        Ok(())
    }

    fn validate_capture_binding(&self) -> Result<(), SnapshotManifestError> {
        if self.version != 3 {
            if self.binding.is_some() || self.logical_name.is_some() {
                return Err(SnapshotManifestError::LegacyBindingFields);
            }
            return Ok(());
        }
        if self.binding.is_none() {
            return Err(SnapshotManifestError::MissingBinding);
        }
        if !is_opaque_id(self.reference.id.as_str()) {
            return Err(SnapshotManifestError::InvalidReferenceId);
        }
        let name = self
            .logical_name
            .as_deref()
            .ok_or(SnapshotManifestError::MissingLogicalName)?;
        validate_logical_name(name)?;
        Ok(())
    }

    /// The immutable binding to restore from; historical manifests have none.
    pub(crate) fn restore_binding(&self) -> Result<&SnapshotBinding, SnapshotManifestError> {
        if self.version != 3 {
            return Err(SnapshotManifestError::LegacyRestore);
        }
        self.binding.as_ref().ok_or(SnapshotManifestError::MissingBinding)
    }
}
