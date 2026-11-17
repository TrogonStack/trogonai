//! Idempotent create-or-validate provisioning of the audit streams.
//!
//! Provisioning is an operator or dev-compose step and never a writer's: a
//! writer holds publish permission on `platform.v1.audit.>` and nothing else
//! (ADR#0086 Decision 2), so a process that created its own stream at start
//! would need a permission that lets it also reshape the stream.
//!
//! The stream is fetched first because JetStream's `STREAM.CREATE` can accept a
//! create against an existing stream as an in-place update, which would let a
//! caller that only meant to check the stream rewrite it, and a weakened audit
//! stream silently repaired to look right is still a gap in the trail. When the
//! stream is missing it is created, and a create that loses a race to another
//! provisioner falls back to a get. Either way an existing stream is validated
//! by [`AuditStream`], which names the field that diverged. Nothing is ever
//! updated: a stream that fails validation is for an operator to decide about.

use std::num::NonZeroUsize;

use async_nats::jetstream::context::CreateStreamError;
use async_nats::jetstream::{self, stream::Stream};
use trogon_nats::jetstream::{is_create_stream_already_exists, is_get_stream_not_found};

use crate::audit_stream::AuditStream;
use crate::ensure_audit_stream_error::EnsureAuditStreamError;

/// Creates `stream` with its required configuration, or validates it if it exists.
pub async fn ensure_audit_stream(
    js: &jetstream::Context,
    stream: AuditStream,
    replicas: NonZeroUsize,
) -> Result<Stream, EnsureAuditStreamError> {
    match js.get_stream(stream.name()).await {
        Ok(existing) => return validated(stream, replicas, existing),
        Err(source) if is_get_stream_not_found(&source) => {}
        Err(source) => {
            return Err(EnsureAuditStreamError::Get {
                stream: stream.name(),
                source,
            });
        }
    }
    let created = js.create_stream(stream.config(replicas)).await;
    settle_create(js, stream, replicas, created).await
}

/// Provisions every audit stream, `AUDIT` first.
pub async fn ensure_audit_streams(
    js: &jetstream::Context,
    replicas: NonZeroUsize,
) -> Result<Vec<Stream>, EnsureAuditStreamError> {
    let mut streams = Vec::with_capacity(AuditStream::ALL.len());
    for stream in AuditStream::ALL {
        streams.push(ensure_audit_stream(js, stream, replicas).await?);
    }
    Ok(streams)
}

/// The outcome of a create: a conflict means another provisioner created the
/// stream first, so it is read back and validated like any existing stream.
async fn settle_create(
    js: &jetstream::Context,
    stream: AuditStream,
    replicas: NonZeroUsize,
    created: Result<Stream, CreateStreamError>,
) -> Result<Stream, EnsureAuditStreamError> {
    match created {
        Ok(created) => Ok(created),
        Err(source) if is_create_stream_already_exists(&source) => {
            let existing = js
                .get_stream(stream.name())
                .await
                .map_err(|source| EnsureAuditStreamError::Get {
                    stream: stream.name(),
                    source,
                })?;
            validated(stream, replicas, existing)
        }
        Err(source) => Err(EnsureAuditStreamError::Create {
            stream: stream.name(),
            source,
        }),
    }
}

fn validated(stream: AuditStream, replicas: NonZeroUsize, existing: Stream) -> Result<Stream, EnsureAuditStreamError> {
    stream.validate(replicas, &existing.cached_info().config)?;
    Ok(existing)
}

#[cfg(test)]
mod tests;
