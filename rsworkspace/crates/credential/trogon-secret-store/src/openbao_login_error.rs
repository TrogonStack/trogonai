use super::{OpenBaoCallError, OpenBaoSecretStoreConfigError};

/// Why an OpenBao store that logs in could not be built.
///
/// A service that cannot log in at startup refuses to start, per ADR#0023
/// Decision 3, rather than run without a token.
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
