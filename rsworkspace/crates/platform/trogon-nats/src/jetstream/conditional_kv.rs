use super::JetStreamPublishMessage;
use crate::NatsToken;
use async_nats::jetstream::{context, kv, message::PublishMessage};
use bytes::Bytes;
use std::{
    future::{Future, IntoFuture},
    num::{NonZeroU64, NonZeroUsize},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeKvScope {
    bucket: NatsToken,
    stream: String,
    prefix: String,
}
impl NativeKvScope {
    pub fn new(bucket: NatsToken) -> Result<Self, NativeKvCasError> {
        if !bucket
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(NativeKvCasError::InvalidBinding);
        }
        Ok(Self {
            stream: format!("KV_{bucket}"),
            prefix: format!("$KV.{bucket}."),
            bucket,
        })
    }
    pub fn bucket(&self) -> &NatsToken {
        &self.bucket
    }
    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn subject(&self, key: &NativeKvKey) -> String {
        format!("{}{}", self.prefix, key.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeKvKey(String);
impl NativeKvKey {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeKvCasError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 1024
            || value.starts_with('.')
            || value.ends_with('.')
            || value.contains("..")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_/=.".contains(&byte))
        {
            return Err(NativeKvCasError::InvalidKey);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeKvExpectation {
    key: Option<NonZeroU64>,
    global: Option<NonZeroU64>,
}
impl NativeKvExpectation {
    /// Zero is an exact native absence/empty-stream comparison, never a wildcard.
    pub fn from_sequences(expected_key: u64, expected_global: u64) -> Result<Self, NativeKvCasError> {
        if expected_key > expected_global || expected_global == u64::MAX {
            return Err(NativeKvCasError::InvalidExpectation);
        }
        Ok(Self {
            key: NonZeroU64::new(expected_key),
            global: NonZeroU64::new(expected_global),
        })
    }
    pub fn key_sequence(self) -> u64 {
        self.key.map_or(0, NonZeroU64::get)
    }
    pub fn global_sequence(self) -> u64 {
        self.global.map_or(0, NonZeroU64::get)
    }
}

pub struct NativeKvUpdate<'a> {
    scope: &'a NativeKvScope,
    key: &'a NativeKvKey,
    value: Bytes,
    expectation: NativeKvExpectation,
    maximum_envelope_bytes: NonZeroUsize,
}
impl<'a> NativeKvUpdate<'a> {
    pub fn new(
        scope: &'a NativeKvScope,
        key: &'a NativeKvKey,
        value: Bytes,
        expectation: NativeKvExpectation,
        maximum_envelope_bytes: NonZeroUsize,
    ) -> Self {
        Self {
            scope,
            key,
            value,
            expectation,
            maximum_envelope_bytes,
        }
    }
    pub fn scope(&self) -> &NativeKvScope {
        self.scope
    }
    pub fn key(&self) -> &NativeKvKey {
        self.key
    }
    pub fn value(&self) -> &Bytes {
        &self.value
    }
    pub fn expectation(&self) -> NativeKvExpectation {
        self.expectation
    }
    pub fn maximum_envelope_bytes(&self) -> NonZeroUsize {
        self.maximum_envelope_bytes
    }
}

#[derive(Clone, Copy, Debug)]
pub enum NativeKvCasStage {
    Publish,
    Acknowledge,
}

#[derive(thiserror::Error)]
pub enum NativeKvCasError {
    #[error("conditional KV scope is not an exact native bucket binding")]
    InvalidBinding,
    #[error("conditional KV key is outside its bounded native profile")]
    InvalidKey,
    #[error("conditional KV key/global sequence expectation is inconsistent")]
    InvalidExpectation,
    #[error("conditional KV message exceeds its envelope limit")]
    EnvelopeLimit,
    #[error("conditional KV global or key sequence changed")]
    Conflict,
    #[error("conditional KV acknowledgement does not prove the exact write")]
    InvalidAcknowledgement,
    #[error("conditional KV publication is uncertain at {stage:?}")]
    Uncertain {
        stage: NativeKvCasStage,
        #[source]
        source: context::PublishError,
    },
}
impl std::fmt::Debug for NativeKvCasError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncertain { stage, .. } => formatter
                .debug_struct("Uncertain")
                .field("stage", stage)
                .finish_non_exhaustive(),
            _ => write!(formatter, "{self}"),
        }
    }
}

/// Only the native conditional publish acknowledges a write. The caller owns
/// capacity, durability, epoch and authorization checks for its exact snapshot.
pub trait JetStreamConditionalKvUpdate: Send + Sync + Clone + 'static {
    fn conditional_update<P>(
        &self,
        publisher: &P,
        request: NativeKvUpdate<'_>,
    ) -> impl Future<Output = Result<NonZeroU64, NativeKvCasError>> + Send
    where
        P: JetStreamPublishMessage<PublishError = context::PublishError>;
}
impl JetStreamConditionalKvUpdate for kv::Store {
    async fn conditional_update<P>(
        &self,
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
        if self.name != scope.bucket.as_str()
            || self.stream_name != scope.stream
            || self.prefix != scope.prefix
            || self.put_prefix.is_some()
            || self.use_jetstream_prefix
            || self.stream.cached_info().config.name != scope.stream
        {
            return Err(NativeKvCasError::InvalidBinding);
        }
        let expected_sequence = expectation
            .global_sequence()
            .checked_add(1)
            .ok_or(NativeKvCasError::InvalidExpectation)?;
        let message = PublishMessage::build()
            .payload(value)
            .expected_stream(&scope.stream)
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
        if ack.duplicate || ack.stream != scope.stream || ack.sequence != expected_sequence {
            return Err(NativeKvCasError::InvalidAcknowledgement);
        }
        NonZeroU64::new(ack.sequence).ok_or(NativeKvCasError::InvalidAcknowledgement)
    }
}

fn classify(source: context::PublishError, stage: NativeKvCasStage) -> NativeKvCasError {
    if source.kind() == context::PublishErrorKind::WrongLastSequence {
        NativeKvCasError::Conflict
    } else {
        NativeKvCasError::Uncertain { stage, source }
    }
}
fn envelope_bytes(message: &async_nats::jetstream::message::OutboundMessage) -> Option<usize> {
    let mut bytes = message.subject.as_str().len().checked_add(message.payload.len())?;
    if let Some(headers) = &message.headers {
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
    }
    Some(bytes)
}

#[cfg(test)]
mod tests;
