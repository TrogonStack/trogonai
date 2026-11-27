use std::time::{SystemTime, UNIX_EPOCH};

use trogon_sandbox_contract::{NativeSnapshotRef, ProviderKind};

use super::reference_error::SnapshotReferenceError;

/// A reference that names an E2B snapshot that has not expired.
pub(crate) fn validate_reference(reference: &NativeSnapshotRef) -> Result<(), SnapshotReferenceError> {
    validate_reference_identity(reference)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if reference.is_expired_at(now) {
        return Err(SnapshotReferenceError::Expired);
    }
    Ok(())
}

/// A reference that names an E2B snapshot, expired or not. Deletion accepts expired references.
pub(crate) fn validate_reference_identity(reference: &NativeSnapshotRef) -> Result<(), SnapshotReferenceError> {
    if reference.provider != ProviderKind::E2b || matches!(reference.id.as_str(), "." | "..") {
        return Err(SnapshotReferenceError::NotE2b);
    }
    Ok(())
}
