use std::ops::Deref;

use trogon_credential_aggregate::CredentialProtoDecodeError;
use trogon_credential_domain::{
    CredentialId, CredentialIdentity, CredentialKey, CredentialMetadata, CredentialRef, CredentialStatus,
    CredentialVersion,
};
use trogon_decider_runtime::{Evolve, StateSnapshot};
use trogonai_proto::gateway::credentials::{state_v1, v1};

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq)]
pub struct CredentialState(state_v1::CredentialStateSnapshot);

impl Default for CredentialState {
    fn default() -> Self {
        Self(state_v1::CredentialStateSnapshot {
            state: Some(state_v1::CredentialMissingState {}.into()),
        })
    }
}

impl Evolve<v1::CredentialEvent> for CredentialState {
    type Error = trogon_credential_aggregate::CredentialEvolveError;

    fn evolve(self, event: &v1::CredentialEvent) -> Result<Self, Self::Error> {
        trogon_credential_aggregate::evolve(self.0, event).map(Self)
    }
}

impl StateSnapshot for CredentialState {
    type Payload = state_v1::CredentialStateSnapshot;
}

impl Deref for CredentialState {
    type Target = state_v1::CredentialStateSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<state_v1::CredentialStateSnapshot> for CredentialState {
    fn as_ref(&self) -> &state_v1::CredentialStateSnapshot {
        &self.0
    }
}

impl From<state_v1::CredentialStateSnapshot> for CredentialState {
    fn from(value: state_v1::CredentialStateSnapshot) -> Self {
        Self(value)
    }
}

impl From<CredentialState> for state_v1::CredentialStateSnapshot {
    fn from(value: CredentialState) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialDecideError {
    #[error("credential state snapshot carried no state")]
    MissingState,
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
    #[error("credential '{credential_id}' is not revoked or expired")]
    CredentialNotRevokedOrExpired { credential_id: CredentialId },
    #[error("credential '{credential_id}' destroy is already requested")]
    CredentialDestroyAlreadyRequested { credential_id: CredentialId },
    #[error("credential '{credential_id}' destroy is not pending")]
    CredentialDestroyNotPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' was already destroyed")]
    CredentialAlreadyDestroyed { credential_id: CredentialId },
    #[error("credential '{key}' was recorded with another identity")]
    IdentityMismatch { key: CredentialKey },
    #[error(
        "credential version does not match the recorded version: expected {}, got {}",
        .expected.get(),
        .actual.get()
    )]
    VersionMismatch {
        expected: CredentialVersion,
        actual: CredentialVersion,
    },
    #[error("credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
    #[error("persisted credential state is invalid: {0}")]
    InvalidProto(#[from] CredentialProtoDecodeError),
}

pub(crate) fn ensure_active_metadata(metadata: &CredentialMetadata) -> Result<(), CredentialDecideError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialDecideError::MetadataNotActive { status });
    }
    Ok(())
}

/// Holds a command to the identity its stream recorded.
pub(crate) fn ensure_recorded_identity(
    recorded: &CredentialIdentity,
    command: &CredentialIdentity,
) -> Result<(), CredentialDecideError> {
    if recorded != command {
        return Err(CredentialDecideError::IdentityMismatch { key: command.key() });
    }
    Ok(())
}

/// Holds a command to the identity and version its stream recorded.
pub(crate) fn ensure_recorded_ref(
    recorded: &CredentialRef,
    command: &CredentialRef,
) -> Result<(), CredentialDecideError> {
    ensure_recorded_identity(recorded.identity(), command.identity())?;
    if recorded.version() != command.version() {
        return Err(CredentialDecideError::VersionMismatch {
            expected: recorded.version(),
            actual: command.version(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
