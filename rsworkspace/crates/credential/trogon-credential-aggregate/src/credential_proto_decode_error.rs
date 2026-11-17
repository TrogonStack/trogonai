use crate::{CredentialFieldInvalidError, CredentialFieldPath};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialProtoDecodeError {
    #[error("credential field '{field}' is missing")]
    MissingField { field: CredentialFieldPath },
    #[error("credential field '{field}' is invalid: {reason}")]
    InvalidField {
        field: CredentialFieldPath,
        #[source]
        reason: CredentialFieldInvalidError,
    },
}
