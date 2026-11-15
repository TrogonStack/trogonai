use trogon_credential_domain::{CredentialKey, CredentialVersion};

use crate::CredentialProtoDecodeError;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialEvolveError {
    #[error("credential event envelope carried no event")]
    MissingEvent,
    #[error("credential state snapshot carried no state")]
    MissingState,
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
    #[error("credential destroy was requested without a revoked or cleanup-failed credential")]
    DestroyRequestedWithoutRevokedOrCleanupFailedCredential,
    #[error("credential was destroyed without a pending destroy request")]
    DestroyedWithoutDestroyRequestedCredential,
    #[error("credential destroy failure was recorded without a pending destroy request")]
    DestroyFailedWithoutDestroyRequestedCredential,
    #[error("credential write request for '{key}' carries another identity than the one recorded")]
    IdentityMismatch { key: CredentialKey },
    #[error("event names credential '{actual}', but the state recorded '{expected}'")]
    CredentialKeyMismatch {
        expected: CredentialKey,
        actual: CredentialKey,
    },
    #[error(
        "event names credential version {}, but the state recorded {}",
        .actual.get(),
        .expected.get()
    )]
    VersionMismatch {
        expected: CredentialVersion,
        actual: CredentialVersion,
    },
    #[error("event names credential '{event}', but it was read from the stream of '{stream}'")]
    EventFromAnotherStream {
        stream: CredentialKey,
        event: CredentialKey,
    },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
    #[error("persisted credential state is invalid: {0}")]
    InvalidProto(#[from] CredentialProtoDecodeError),
}
