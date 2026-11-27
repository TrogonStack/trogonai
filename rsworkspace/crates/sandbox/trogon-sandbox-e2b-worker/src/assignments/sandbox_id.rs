use super::assignments_error::AssignmentsError;
use crate::native_sandbox_id::NativeSandboxId;

/// Refuses a native sandbox identity that is not in E2B's canonical form.
pub(crate) fn validate_sandbox_id(id: &str) -> Result<(), AssignmentsError> {
    NativeSandboxId::new(id)?;
    Ok(())
}
