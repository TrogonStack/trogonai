use async_nats::jetstream::context::{CreateStreamError, GetStreamError};

use crate::audit_stream_mismatch_error::AuditStreamMismatchError;

#[derive(Debug, thiserror::Error)]
pub enum EnsureAuditStreamError {
    #[error("failed to create stream '{stream}'")]
    Create {
        stream: &'static str,
        #[source]
        source: CreateStreamError,
    },
    #[error("failed to get existing stream '{stream}'")]
    Get {
        stream: &'static str,
        #[source]
        source: GetStreamError,
    },
    #[error(transparent)]
    ConfigMismatch(#[from] AuditStreamMismatchError),
}
