//! Reading "the resource does not exist" out of a failed JetStream get.
//!
//! async-nats collapses every reason a KV get can fail into
//! [`KeyValueErrorKind::GetBucket`], so a bucket that is absent and a bucket
//! that could not be read are the same value until the wrapped
//! [`GetStreamError`] is unwrapped. Provisioning code that skips that step
//! reads a request timeout or a denied `STREAM.INFO` as absence and then
//! creates over storage that already exists.

use async_nats::jetstream::{
    ErrorCode,
    context::{GetStreamError, GetStreamErrorKind, KeyValueError, KeyValueErrorKind},
};

/// True only when JetStream itself answered that the stream is not there.
pub fn is_get_stream_not_found(error: &GetStreamError) -> bool {
    matches!(
        error.kind(),
        GetStreamErrorKind::JetStream(ref source) if source.error_code() == ErrorCode::STREAM_NOT_FOUND
    )
}

/// True only when JetStream itself answered that the bucket's backing stream is
/// not there. The kind cannot say so on its own, hence the downcast: every get
/// failure carries `GetBucket` and the reason lives in the source.
pub fn is_get_key_value_not_found(error: &KeyValueError) -> bool {
    if error.kind() != KeyValueErrorKind::GetBucket {
        return false;
    }

    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<GetStreamError>())
        .is_some_and(is_get_stream_not_found)
}

#[cfg(test)]
mod tests;
