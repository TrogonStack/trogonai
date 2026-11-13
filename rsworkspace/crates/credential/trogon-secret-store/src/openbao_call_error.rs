use std::time::Duration;

use reqwest::StatusCode;

use super::SecretStoreError;

/// One OpenBao HTTP call that did not succeed.
///
/// The body of a refused call is OpenBao's error list, which never carries a
/// token; the body of a successful call is never kept here, because a login or
/// a token creation answers with one.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenBaoCallError {
    #[error("OpenBao returned {status}: {body}")]
    Status { status: StatusCode, body: String },
    #[error("{0}")]
    Backend(String),
    #[error("failed to build endpoint url: {0}")]
    InvalidEndpoint(#[source] url::ParseError),
    #[error("OpenBao granted a token that expires in {}s, too soon to send", ttl.as_secs())]
    UnusableLease { ttl: Duration },
}

impl OpenBaoCallError {
    pub(crate) fn into_store_error(self) -> SecretStoreError {
        SecretStoreError::BackendUnavailable {
            message: self.to_string(),
        }
    }
}
