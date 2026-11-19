use crate::{RegistrationArchive, RegistrationArchiveProfile, RegistrationArtifact};
use std::{num::NonZeroU64, sync::Arc, time::SystemTime};

/// Neutral native evidence with no public construction or authority conversion.
///
/// ```compile_fail
/// use trogon_workload_registration::{RegistrationArchive, RegistrationArtifact, RegistrationArchiveReceipt};
/// fn forge(artifact: RegistrationArtifact, owner: std::sync::Arc<RegistrationArchive>) -> RegistrationArchiveReceipt {
///     RegistrationArchiveReceipt {
///         artifact,
///         native_revision: std::num::NonZeroU64::MIN,
///         stream_created_at: std::time::SystemTime::UNIX_EPOCH,
///         owner,
///     }
/// }
/// ```
/// ```compile_fail
/// use trogon_workload_registration::{RegistrationArchiveReceipt, RegistrationArtifactObservation};
/// fn forge(observed: RegistrationArtifactObservation) -> RegistrationArchiveReceipt { observed.into() }
/// ```
pub struct RegistrationArchiveReceipt {
    artifact: RegistrationArtifact,
    native_revision: NonZeroU64,
    stream_created_at: SystemTime,
    owner: Arc<RegistrationArchive>,
}
impl RegistrationArchiveReceipt {
    pub(super) fn from_native(
        artifact: RegistrationArtifact,
        native_revision: NonZeroU64,
        stream_created_at: SystemTime,
        owner: Arc<RegistrationArchive>,
    ) -> Self {
        Self {
            artifact,
            native_revision,
            stream_created_at,
            owner,
        }
    }

    pub fn artifact(&self) -> &RegistrationArtifact {
        &self.artifact
    }
    pub fn native_revision(&self) -> NonZeroU64 {
        self.native_revision
    }
    pub fn stream_created_at(&self) -> SystemTime {
        self.stream_created_at
    }
    pub fn source_profile(&self) -> &RegistrationArchiveProfile {
        self.owner.profile()
    }
}
impl std::fmt::Debug for RegistrationArchiveReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistrationArchiveReceipt { .. }")
    }
}
