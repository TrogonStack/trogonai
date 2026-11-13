use super::{JetStreamPublishMessage, NativeKvCasError, NativeKvCasStage, NativeKvUpdate};
use async_nats::jetstream::{Context, context, kv, message::PublishMessage};
use std::{future::IntoFuture, num::NonZeroU64};

/// Only the native conditional publish acknowledges a write. The caller owns
/// capacity, durability, epoch and authorization checks for its exact snapshot.
/// The receiving context both looks up the bucket and publishes the update;
/// an independent publisher cannot provide evidence for another store.
pub async fn conditional_native_kv_update(
    context: &Context,
    request: NativeKvUpdate<'_>,
) -> Result<NonZeroU64, NativeKvCasError> {
    let store = context
        .get_key_value(request.scope().bucket().as_str())
        .await
        .map_err(NativeKvCasError::Lookup)?;
    publish_checked_update(&store, context, request).await
}

async fn publish_checked_update<P>(
    store: &kv::Store,
    publisher: &P,
    request: NativeKvUpdate<'_>,
) -> Result<NonZeroU64, NativeKvCasError>
where
    P: JetStreamPublishMessage<PublishError = context::PublishError>,
{
    let NativeKvUpdate {
        scope,
        key,
        value,
        expectation,
        maximum_envelope_bytes,
    } = request;
    if store.name != scope.bucket().as_str()
        || store.stream_name != scope.stream()
        || store.prefix != format!("$KV.{}.", scope.bucket())
        || store.put_prefix.is_some()
        || store.use_jetstream_prefix
        || store.stream.cached_info().config.name != scope.stream()
    {
        return Err(NativeKvCasError::InvalidBinding);
    }
    let expected_sequence = expectation.next_sequence();
    let message = PublishMessage::build()
        .payload(value)
        .expected_stream(scope.stream())
        .expected_last_sequence(expectation.global_sequence())
        .expected_last_subject_sequence(expectation.key_sequence())
        .outbound_message(scope.subject(key));
    if envelope_bytes(&message).is_none_or(|bytes| bytes > maximum_envelope_bytes.get()) {
        return Err(NativeKvCasError::EnvelopeLimit);
    }
    let pending = publisher
        .publish_message(message)
        .await
        .map_err(|source| classify(source, NativeKvCasStage::Publish))?;
    let ack = pending
        .into_future()
        .await
        .map_err(|source| classify(source, NativeKvCasStage::Acknowledge))?;
    if ack.duplicate || ack.stream != scope.stream() || ack.sequence != expected_sequence.get() {
        return Err(NativeKvCasError::InvalidAcknowledgement);
    }
    Ok(expected_sequence)
}

fn classify(source: context::PublishError, stage: NativeKvCasStage) -> NativeKvCasError {
    if source.kind() == context::PublishErrorKind::WrongLastSequence {
        NativeKvCasError::Conflict
    } else {
        NativeKvCasError::Uncertain { stage, source }
    }
}
fn envelope_bytes(message: &async_nats::jetstream::message::OutboundMessage) -> Option<usize> {
    let headers = message.headers.as_ref()?;
    let mut bytes = message.subject.as_str().len().checked_add(message.payload.len())?;
    bytes = bytes.checked_add(b"NATS/1.0\r\n\r\n".len())?;
    for (key, values) in headers.iter() {
        for value in values {
            bytes = bytes
                .checked_add(AsRef::<str>::as_ref(key).len())?
                .checked_add(b": ".len())?
                .checked_add(value.as_str().len())?
                .checked_add(b"\r\n".len())?;
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests;
