use super::domain::{
    CredentialFailureReason, CredentialId, CredentialKind, CredentialLifecycleEvent, CredentialMetadata,
    CredentialOwnerId, CredentialRef, CredentialStatus, SourceKind,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialLifecycleState {
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
pub enum CredentialLifecycleDecideError {
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
    #[error("credential ref does not match lifecycle stream: expected '{expected}', got '{actual}'")]
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
pub enum CredentialLifecycleEvolveError {
    #[error("credential write was requested after lifecycle already started")]
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
    #[error("event credential ref does not match lifecycle stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("event credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
}

pub fn initial_state() -> CredentialLifecycleState {
    CredentialLifecycleState::Missing
}

pub fn evolve(
    state: CredentialLifecycleState,
    event: &CredentialLifecycleEvent,
) -> Result<CredentialLifecycleState, CredentialLifecycleEvolveError> {
    match event {
        CredentialLifecycleEvent::WriteRequested {
            credential_id,
            owner_id,
            source,
            kind,
        } => match state {
            CredentialLifecycleState::Missing => Ok(CredentialLifecycleState::PendingWrite(PendingCredentialWrite {
                credential_id: credential_id.clone(),
                owner_id: owner_id.clone(),
                source: *source,
                kind: *kind,
            })),
            _ => Err(CredentialLifecycleEvolveError::WriteRequestedAfterStart),
        },
        CredentialLifecycleEvent::WriteFailed { credential_id, reason } => match state {
            CredentialLifecycleState::PendingWrite(pending) if &pending.credential_id == credential_id => {
                Ok(CredentialLifecycleState::WriteFailed(FailedCredentialWrite {
                    credential_id: credential_id.clone(),
                    reason: reason.clone(),
                }))
            }
            CredentialLifecycleState::PendingWrite(pending) => {
                Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
                    expected: pending.credential_id,
                    actual: credential_id.clone(),
                })
            }
            _ => Err(CredentialLifecycleEvolveError::WriteFailedWithoutPendingWrite),
        },
        CredentialLifecycleEvent::Activated { metadata } => match state {
            CredentialLifecycleState::PendingWrite(pending) => {
                validate_activation_metadata_for_evolve(metadata)?;
                validate_ref_matches_pending_for_evolve(metadata.reference(), &pending)?;
                Ok(CredentialLifecycleState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions: Vec::new(),
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::ActivatedWithoutPendingWrite),
        },
        CredentialLifecycleEvent::RotationRequested { credential_ref } => match state {
            CredentialLifecycleState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::RotationPending(RotationPendingCredential {
                    active,
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RotationRequestedWithoutActiveCredential),
        },
        CredentialLifecycleEvent::RotationFailed { credential_ref, .. } => match state {
            CredentialLifecycleState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::Active(rotation.active))
            }
            _ => Err(CredentialLifecycleEvolveError::RotationFailedWithoutPendingRotation),
        },
        CredentialLifecycleEvent::Rotated {
            previous_credential_ref,
            metadata,
        } => match state {
            CredentialLifecycleState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), previous_credential_ref)?;
                validate_activation_metadata_for_evolve(metadata)?;
                validate_same_logical_ref_for_evolve(previous_credential_ref, metadata.reference())?;
                validate_newer_version_for_evolve(previous_credential_ref, metadata.reference())?;
                let mut previous_versions = rotation.active.previous_versions;
                previous_versions.push(previous_credential_ref.clone());
                Ok(CredentialLifecycleState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions,
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RotatedWithoutPendingRotation),
        },
        CredentialLifecycleEvent::Revoked { credential_ref } => match state {
            CredentialLifecycleState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::Revoked(RevokedCredential {
                    credential_ref: credential_ref.clone(),
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RevokedWithoutActiveCredential),
        },
    }
}

pub(crate) fn validate_activation_metadata(
    metadata: &CredentialMetadata,
) -> Result<(), CredentialLifecycleDecideError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialLifecycleDecideError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_activation_metadata_for_evolve(
    metadata: &CredentialMetadata,
) -> Result<(), CredentialLifecycleEvolveError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialLifecycleEvolveError::MetadataNotActive { status });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialLifecycleDecideError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_ref_matches_pending_for_evolve(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialLifecycleEvolveError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleDecideError> {
    if expected != actual {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if expected != actual {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleDecideError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_same_logical_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

pub(crate) fn validate_newer_version(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialLifecycleDecideError> {
    if next.version() <= current.version() {
        return Err(CredentialLifecycleDecideError::RotationVersionNotNewer);
    }
    Ok(())
}

pub(crate) fn validate_newer_version_for_evolve(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if next.version() <= current.version() {
        return Err(CredentialLifecycleEvolveError::RotationVersionNotNewer);
    }
    Ok(())
}
