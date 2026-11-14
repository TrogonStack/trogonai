use std::num::NonZeroU64;

use crate::{RegistrationArchiveError, RegistrationArtifact};

pub(super) fn require_original_evidence(
    expected: &RegistrationArtifact,
    observed: &RegistrationArtifact,
    native_revision: NonZeroU64,
    acknowledged_revision: Option<NonZeroU64>,
) -> Result<(), RegistrationArchiveError> {
    if observed != expected || acknowledged_revision.is_some_and(|expected| native_revision != expected) {
        return Err(RegistrationArchiveError::Integrity);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
