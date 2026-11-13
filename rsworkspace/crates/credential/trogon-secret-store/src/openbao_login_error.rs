use super::{OpenBaoCallError, OpenBaoSecretStoreConfigError};

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
}

impl OpenBaoLoginError {
    pub(crate) fn refused(error: OpenBaoCallError) -> Self {
        Self::Refused {
            message: error.to_string(),
        }
    }
}
