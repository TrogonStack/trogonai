use crate::{ProducerConfig, RegistrationArchiveCapacity, RegistrationArchiveError, RegistrationArtifactKind};

/// Deployment-selected scope and limits; neither a verified account nor delegation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationArchiveConfig {
    registration: ProducerConfig,
    approval: RegistrationArchiveCapacity,
    manifest: RegistrationArchiveCapacity,
}
impl RegistrationArchiveConfig {
    pub fn new(
        registration: ProducerConfig,
        approval: RegistrationArchiveCapacity,
        manifest: RegistrationArchiveCapacity,
    ) -> Result<Self, RegistrationArchiveError> {
        if approval.kind() != RegistrationArtifactKind::OriginalApproval
            || manifest.kind() != RegistrationArtifactKind::BootstrapManifest
        {
            return Err(RegistrationArchiveError::Capacity);
        }
        Ok(Self {
            registration,
            approval,
            manifest,
        })
    }
    pub fn registration(&self) -> &ProducerConfig {
        &self.registration
    }
    pub fn capacity(&self, kind: RegistrationArtifactKind) -> RegistrationArchiveCapacity {
        match kind {
            RegistrationArtifactKind::OriginalApproval => self.approval,
            RegistrationArtifactKind::BootstrapManifest => self.manifest,
        }
    }
}
