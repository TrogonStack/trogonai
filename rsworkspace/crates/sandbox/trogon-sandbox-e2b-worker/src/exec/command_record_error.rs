use super::command_owner_error::CommandOwnerError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CommandRecordError {
    #[error("unsupported command custody version")]
    Version,
    #[error(transparent)]
    Owner(#[from] CommandOwnerError),
    #[error("invalid host command execution identity: {0}")]
    ExecutionIdentity(#[from] crate::ulid_error::UlidError),
    #[error("command custody phase lacks immutable authority")]
    PhaseAuthority,
    #[error("command custody has an invalid native binding")]
    NativeBinding,
    #[error("command custody cannot be encoded: {0}")]
    Encode(#[from] serde_json::Error),
}
