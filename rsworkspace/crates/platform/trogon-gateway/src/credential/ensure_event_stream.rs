//! Idempotent create-or-validate provisioning of the credential event stream.
//!
//! The stream is fetched first because JetStream's `STREAM.CREATE` can accept a
//! create against an existing stream as an in-place update, which would let a
//! gateway that only meant to open the stream rewrite it. When the stream is
//! missing it is created, and a create that loses a race to another gateway
//! falls back to a get. Either way an existing stream is validated, and nothing
//! is ever updated: a stream that fails validation is for an operator to decide
//! about.

use async_nats::jetstream::context::CreateStreamError;
use async_nats::jetstream::{self, stream::Stream};
use trogon_nats::jetstream::{is_create_stream_already_exists, is_get_stream_not_found};

use super::constants::CREDENTIAL_STREAM;
use super::ensure_event_stream_error::EnsureCredentialEventStreamError;
use super::stream::{stream_config, validate_stream_config};

/// Creates the credential event stream, or validates it if it exists.
pub(crate) async fn ensure_credential_event_stream(
    js: &jetstream::Context,
) -> Result<Stream, EnsureCredentialEventStreamError> {
    match js.get_stream(CREDENTIAL_STREAM).await {
        Ok(existing) => return validated(existing),
        Err(source) if is_get_stream_not_found(&source) => {}
        Err(source) => return Err(EnsureCredentialEventStreamError::Get { source }),
    }
    let created = js.create_stream(stream_config()).await;
    settle_create(js, created).await
}

/// The outcome of a create: a conflict means another gateway created the stream
/// first, so it is read back and validated like any existing stream.
async fn settle_create(
    js: &jetstream::Context,
    created: Result<Stream, CreateStreamError>,
) -> Result<Stream, EnsureCredentialEventStreamError> {
    match created {
        Ok(created) => Ok(created),
        Err(source) if is_create_stream_already_exists(&source) => {
            let existing = js
                .get_stream(CREDENTIAL_STREAM)
                .await
                .map_err(|source| EnsureCredentialEventStreamError::Get { source })?;
            validated(existing)
        }
        Err(source) => Err(EnsureCredentialEventStreamError::Create { source }),
    }
}

fn validated(existing: Stream) -> Result<Stream, EnsureCredentialEventStreamError> {
    validate_stream_config(&existing.cached_info().config)?;
    Ok(existing)
}

#[cfg(test)]
mod tests;
