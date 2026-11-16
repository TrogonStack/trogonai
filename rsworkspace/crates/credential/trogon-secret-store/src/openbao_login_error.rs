use super::{OpenBaoAppRoleError, OpenBaoCallError, OpenBaoSecretStoreConfigError};

/// Why an OpenBao store that logs in could not be built.
///
/// A service never serves without a token (ADR#0023 Decision 3): it retries a
/// refused login with backoff, and refuses to start on a configuration error,
/// which no retry can fix.
#[derive(Debug, thiserror::Error)]
pub enum OpenBaoLoginError {
    #[error(transparent)]
    Config(#[from] OpenBaoSecretStoreConfigError),
    #[error("OpenBao refused the AppRole login: {message}")]
    Refused { message: String },
    /// The secret id file could not be read for this login. The platform may
    /// still be writing a rotated secret id, so a retry can succeed.
    #[error(transparent)]
    SecretId(OpenBaoAppRoleError),
}

impl OpenBaoLoginError {
    pub(crate) fn failed(error: OpenBaoCallError) -> Self {
        match error {
            OpenBaoCallError::SecretId(error) => Self::SecretId(error),
            error => Self::Refused {
                message: error.to_string(),
            },
        }
    }
}
