use super::constants::MAX_LOGICAL_NAME_BYTES;
use super::logical_name_error::SnapshotLogicalNameError;

/// A logical snapshot name is at most 128 bytes with no control characters.
pub(crate) fn validate_logical_name(name: &str) -> Result<(), SnapshotLogicalNameError> {
    if name.len() > MAX_LOGICAL_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(SnapshotLogicalNameError::Invalid);
    }
    Ok(())
}
