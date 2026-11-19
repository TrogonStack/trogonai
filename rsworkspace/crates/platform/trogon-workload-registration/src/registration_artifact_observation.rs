use crate::{RegistrationArchive, RegistrationArchiveProfile, RegistrationArtifact};
use std::{num::NonZeroU64, sync::Arc, time::SystemTime};

/// Neutral native evidence with no public construction or authority conversion.
///
/// ```compile_fail
/// use trogon_workload_registration::{RegistrationArchive, RegistrationArtifact, RegistrationArtifactObservation};
/// fn forge(artifact: RegistrationArtifact, owner: std::sync::Arc<RegistrationArchive>) -> RegistrationArtifactObservation {
///     RegistrationArtifactObservation {
///         artifact,
///         native_revision: std::num::NonZeroU64::MIN,
///         stream_created_at: std::time::SystemTime::UNIX_EPOCH,
///         owner,
///     }
/// }
/// ```
pub struct RegistrationArtifactObservation {
    artifact: RegistrationArtifact,
    native_revision: NonZeroU64,
    stream_created_at: SystemTime,
    owner: Arc<RegistrationArchive>,
}
impl RegistrationArtifactObservation {
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

    /// A new bounded source observation, not renewal of any authority grant.
    pub async fn revalidate_original(&self) -> Result<(), crate::RegistrationArchiveError> {
        self.owner.revalidate_original(self).await
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
impl std::fmt::Debug for RegistrationArtifactObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistrationArtifactObservation { .. }")
    }
}
