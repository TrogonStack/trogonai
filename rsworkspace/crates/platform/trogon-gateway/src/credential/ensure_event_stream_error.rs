use async_nats::jetstream::context::{CreateStreamError, GetStreamError};

use super::constants::CREDENTIAL_STREAM;
use super::event_stream_mismatch_error::CredentialEventStreamMismatchError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EnsureCredentialEventStreamError {
    #[error("failed to create stream '{CREDENTIAL_STREAM}'")]
    Create {
        #[source]
        source: CreateStreamError,
    },
    #[error("failed to get existing stream '{CREDENTIAL_STREAM}'")]
    Get {
        #[source]
        source: GetStreamError,
    },
    #[error(transparent)]
    ConfigMismatch(#[from] CredentialEventStreamMismatchError),
}
