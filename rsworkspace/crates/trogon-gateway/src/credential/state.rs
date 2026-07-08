use super::domain::{
    CredentialEvent, CredentialFailureReason, CredentialId, CredentialKind, CredentialMetadata, CredentialOwnerId,
    CredentialRef, CredentialStatus, SourceKind,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialState {
    Missing,
    PendingWrite(PendingCredentialWrite),
    Active(ActiveCredential),
    WriteFailed(FailedCredentialWrite),
    RotationPending(RotationPendingCredential),
    Revoked(RevokedCredential),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCredentialWrite {
    pub(super) credential_id: CredentialId,
    pub(super) owner_id: CredentialOwnerId,
    pub(super) source: SourceKind,
    pub(super) kind: CredentialKind,
}

impl PendingCredentialWrite {
    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCredential {
    pub(super) metadata: CredentialMetadata,
    pub(super) previous_versions: Vec<CredentialRef>,
}

impl ActiveCredential {
    pub fn metadata(&self) -> &CredentialMetadata {
        &self.metadata
    }

    pub fn credential_ref(&self) -> &CredentialRef {
        self.metadata.reference()
    }

    pub fn previous_versions(&self) -> &[CredentialRef] {
        &self.previous_versions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedCredentialWrite {
    pub(super) credential_id: CredentialId,
    pub(super) reason: CredentialFailureReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationPendingCredential {
    pub(super) active: ActiveCredential,
}

impl RotationPendingCredential {
    pub fn active(&self) -> &ActiveCredential {
        &self.active
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokedCredential {
    pub(super) credential_ref: CredentialRef,
}

impl RevokedCredential {
    pub fn credential_ref(&self) -> &CredentialRef {
        &self.credential_ref
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialDecideError {
    #[error("credential '{credential_id}' already exists")]
    AlreadyExists { credential_id: CredentialId },
    #[error("credential '{credential_id}' was revoked")]
    Revoked { credential_id: CredentialId },
    #[error("credential '{credential_id}' does not exist")]
    CredentialNotFound { credential_id: CredentialId },
    #[error("credential '{credential_id}' write is not pending")]
    CredentialWriteNotPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' is not active")]
    CredentialNotActive { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is already pending")]
    CredentialRotationAlreadyPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is not pending")]
    CredentialRotationNotPending { credential_id: CredentialId },
    #[error("credential ref does not match credential stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialEvolveError {
    #[error("credential write was requested after write already started")]
    WriteRequestedAfterStart,
    #[error("credential write failure was recorded without a pending write")]
    WriteFailedWithoutPendingWrite,
    #[error("credential was activated without a pending write")]
    ActivatedWithoutPendingWrite,
    #[error("credential rotation was requested without an active credential")]
    RotationRequestedWithoutActiveCredential,
    #[error("credential rotation failure was recorded without a pending rotation")]
    RotationFailedWithoutPendingRotation,
    #[error("credential was rotated without a pending rotation")]
    RotatedWithoutPendingRotation,
    #[error("credential was revoked without an active credential")]
    RevokedWithoutActiveCredential,
    #[error("event credential ref does not match credential stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("event credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
}

pub fn initial_state() -> CredentialState {
    CredentialState::Missing
}

pub fn evolve(state: CredentialState, event: &CredentialEvent) -> Result<CredentialState, CredentialEvolveError> {
    match event {
        CredentialEvent::WriteRequested {
            credential_id,
            owner_id,
            source,
            kind,
        } => match state {
            CredentialState::Missing => Ok(CredentialState::PendingWrite(PendingCredentialWrite {
                credential_id: credential_id.clone(),
                owner_id: owner_id.clone(),
                source: *source,
                kind: *kind,
            })),
            _ => Err(CredentialEvolveError::WriteRequestedAfterStart),
        },
        CredentialEvent::WriteFailed { credential_id, reason } => match state {
            CredentialState::PendingWrite(pending) if &pending.credential_id == credential_id => {
                Ok(CredentialState::WriteFailed(FailedCredentialWrite {
                    credential_id: credential_id.clone(),
                    reason: reason.clone(),
                }))
            }
            CredentialState::PendingWrite(pending) => Err(CredentialEvolveError::CredentialRefMismatch {
                expected: pending.credential_id,
                actual: credential_id.clone(),
            }),
            _ => Err(CredentialEvolveError::WriteFailedWithoutPendingWrite),
        },
        CredentialEvent::Activated { metadata } => match state {
            CredentialState::PendingWrite(pending) => {
                validate_activation_metadata_for_evolve(metadata)?;
                validate_ref_matches_pending_for_evolve(metadata.reference(), &pending)?;
                Ok(CredentialState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions: Vec::new(),
                }))
            }
            _ => Err(CredentialEvolveError::ActivatedWithoutPendingWrite),
        },
        CredentialEvent::RotationRequested { credential_ref } => match state {
            CredentialState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialState::RotationPending(RotationPendingCredential { active }))
            }
            _ => Err(CredentialEvolveError::RotationRequestedWithoutActiveCredential),
        },
        CredentialEvent::RotationFailed { credential_ref, .. } => match state {
            CredentialState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), credential_ref)?;
                Ok(CredentialState::Active(rotation.active))
            }
            _ => Err(CredentialEvolveError::RotationFailedWithoutPendingRotation),
        },
        CredentialEvent::Rotated {
            previous_credential_ref,
            metadata,
        } => match state {
            CredentialState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), previous_credential_ref)?;
                validate_activation_metadata_for_evolve(metadata)?;
                validate_same_logical_ref_for_evolve(previous_credential_ref, metadata.reference())?;
                validate_newer_version_for_evolve(previous_credential_ref, metadata.reference())?;
                let mut previous_versions = rotation.active.previous_versions;
                previous_versions.push(previous_credential_ref.clone());
                Ok(CredentialState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions,
                }))
            }
            _ => Err(CredentialEvolveError::RotatedWithoutPendingRotation),
        },
        CredentialEvent::Revoked { credential_ref } => match state {
            CredentialState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialState::Revoked(RevokedCredential {
                    credential_ref: credential_ref.clone(),
                }))
            }
            _ => Err(CredentialEvolveError::RevokedWithoutActiveCredential),
        },
    }
}

pub(crate) fn validate_activation_metadata(metadata: &CredentialMetadata) -> Result<(), CredentialDecideError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialDecideError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_activation_metadata_for_evolve(
    metadata: &CredentialMetadata,
) -> Result<(), CredentialEvolveError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialEvolveError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialDecideError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending_for_evolve(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialEvolveError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref(expected: &CredentialRef, actual: &CredentialRef) -> Result<(), CredentialDecideError> {
    if expected != actual {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if expected != actual {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialDecideError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_newer_version(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialDecideError> {
    if next.version() <= current.version() {
        return Err(CredentialDecideError::RotationVersionNotNewer);
    }
    Ok(())
}

pub(crate) fn validate_newer_version_for_evolve(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialEvolveError> {
    if next.version() <= current.version() {
        return Err(CredentialEvolveError::RotationVersionNotNewer);
    }
    Ok(())
}
