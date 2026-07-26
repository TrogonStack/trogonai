//! Event stream storage helpers backed by JetStream streams.
//!
//! Events are persisted as JetStream messages on caller-resolved subjects.
//! Trogon event metadata is stored in headers so the payload remains the
//! encoded domain event body produced by `trogon-decider-runtime`.

use async_nats::{
    HeaderMap, Subject, SubjectError,
    header::{HeaderName, HeaderValue, IntoHeaderName, NATS_MESSAGE_ID, ParseHeaderNameError, ParseHeaderValueError},
    jetstream::{
        self, Error as JetStreamError, ErrorCode, context,
        context::PublishErrorKind,
        message::PublishMessage,
        publish::PublishAck,
        stream::{InfoError, LastRawMessageError},
    },
    subject::ToSubject,
};
use chrono::{DateTime, Utc};
use std::{fmt, future::IntoFuture};
use trogon_decider_runtime::{
    Event, EventId, FromEntriesError, Headers, InvalidStreamPositionError, StreamEvent, StreamPosition,
};
use trogon_nats::jetstream::{JetStreamLastRawMessageBySubject, JetStreamPublishMessage};
use trogon_std::{NowV7, UuidV7Generator};

mod replay;

type StreamMessage = async_nats::jetstream::message::StreamMessage;

const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
const NATS_BATCH_ID: &str = "Nats-Batch-Id";
const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";
/// Prefix used to encode user event headers into NATS message headers.
pub const TROGON_EVENT_HEADER_PREFIX: &str = "Trogon-Header-";
/// Header that stores the domain event type.
pub const TROGON_EVENT_TYPE: &str = "Trogon-Event-Type";

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validated JetStream subject used by the decider storage adapter.
pub struct StreamSubject(Subject);

impl StreamSubject {
    /// Creates a subject after applying `async-nats` subject validation.
    pub fn new(subject: impl AsRef<str>) -> Result<Self, SubjectError> {
        Subject::validated(subject).map(Self)
    }

    /// Returns the validated subject as a string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for StreamSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ToSubject for StreamSubject {
    fn to_subject(&self) -> Subject {
        self.0.clone()
    }
}

impl std::str::FromStr for StreamSubject {
    type Err = SubjectError;

    fn from_str(subject: &str) -> Result<Self, Self::Err> {
        Self::new(subject)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Resolved JetStream subject state for a decider stream.
pub struct SubjectState {
    /// JetStream subject used to store events for the requested stream.
    pub subject: StreamSubject,
    /// Latest recorded position for the subject, if the stream already exists.
    pub current_position: Option<StreamPosition>,
}

/// Resolves a domain stream identifier to the JetStream subject that stores it.
///
/// Implementations usually compose a tenant or aggregate prefix with the
/// caller's stream id, then use [`subject_current_position`] to fetch the
/// subject's latest sequence. Keeping this resolver outside [`crate::JetStreamStore`]
/// lets applications own their subject naming scheme.
pub trait StreamSubjectResolver<StreamId: ?Sized>: Send + Sync + Clone + 'static {
    /// Error raised while resolving a stream subject or reading its state.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolves the stream id into a subject and current position.
    fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<SubjectState, Self::Error>> + Send;
}

#[derive(Debug, thiserror::Error)]
/// Error raised while reading stream events.
#[allow(missing_docs)]
pub enum ReadStreamError {
    #[error("failed to query stream info: {source}")]
    QueryStreamInfo {
        #[source]
        source: InfoError,
    },
    #[error("failed to read latest subject position: {source}")]
    ReadLatestSubjectPosition {
        #[source]
        source: InvalidStreamPositionError,
    },
    #[error("failed to read latest subject message: {source}")]
    ReadLatestSubjectMessage {
        #[source]
        source: LastRawMessageError,
    },
    #[error("failed to convert stream message timestamp into recorded event time: {subject}")]
    InvalidMessageTimestamp { subject: String },
    #[error("failed to read stream message event id: {source}")]
    ReadMessageEventId {
        #[source]
        source: uuid::Error,
    },
    #[error("failed to read stream message position: {source}")]
    ReadMessagePosition {
        #[source]
        source: InvalidStreamPositionError,
    },
    #[error("failed to read stream message event headers: event header '{header_name}' must have exactly one value")]
    InvalidEventHeaderValueCount { header_name: String },
    #[error("failed to read stream message event headers: {source}")]
    ReadMessageEventHeaders {
        #[source]
        source: FromEntriesError,
    },
    #[error("failed to read stream message event envelope: stream message is missing {header_name} header")]
    MissingMessageHeader { header_name: &'static str },
    #[error("failed to create replay consumer: {source}")]
    CreateReplayConsumer {
        #[source]
        source: async_nats::jetstream::stream::ConsumerError,
    },
    #[error("failed to open replay message stream: {source}")]
    OpenReplayMessageStream {
        #[source]
        source: async_nats::jetstream::consumer::StreamError,
    },
    #[error("failed to read replay message: {source}")]
    ReadReplayMessage {
        #[source]
        source: async_nats::jetstream::consumer::pull::OrderedError,
    },
    #[error("failed to read replay message delivery metadata: {source}")]
    ReadReplayMessageInfo {
        #[source]
        source: async_nats::Error,
    },
    #[error("replay message stream ended before reaching sequence {to_sequence}")]
    ReplayEndedBeforeTarget { to_sequence: u64 },
}

#[derive(Debug, thiserror::Error)]
/// Error raised while appending stream events.
#[allow(missing_docs)]
pub enum PublishStreamError {
    #[error("failed to publish stream event: {source}")]
    PublishEvent {
        #[source]
        source: context::PublishError,
    },
    #[error("failed to publish stream event batch: batch commit ack was not created")]
    MissingBatchCommitAck,
    #[error("failed to acknowledge stream event: {source}")]
    AcknowledgeEvent {
        #[source]
        source: context::PublishError,
    },
    #[error("failed to acknowledge stream event batch commit: {source}")]
    AcknowledgeBatchCommit {
        #[source]
        source: context::PublishError,
    },
    #[error("duplicate event id")]
    DuplicateEventId,
    #[error("failed to read stream append position: {source}")]
    ReadAppendPosition {
        #[source]
        source: InvalidStreamPositionError,
    },
    #[error("failed to encode event header name '{header_name}': {source}")]
    InvalidEventHeaderName {
        header_name: String,
        #[source]
        source: ParseHeaderNameError,
    },
    #[error("failed to encode event header '{header_name}' value: {source}")]
    InvalidEventHeaderValue {
        header_name: String,
        value: String,
        #[source]
        source: ParseHeaderValueError,
    },
}

#[derive(Debug, thiserror::Error)]
/// Error raised while reading or appending stream events.
pub enum StreamStoreError {
    /// JetStream read operation failed.
    #[error("stream read error: {0}")]
    Read(#[source] ReadStreamError),
    /// JetStream publish operation failed.
    #[error("stream publish error: {0}")]
    Publish(#[source] PublishStreamError),
    /// JetStream rejected the expected last subject sequence.
    #[error("stream publish error: wrong expected version")]
    WrongExpectedVersion,
}

impl From<ReadStreamError> for StreamStoreError {
    fn from(source: ReadStreamError) -> Self {
        Self::Read(source)
    }
}

impl From<PublishStreamError> for StreamStoreError {
    fn from(source: PublishStreamError) -> Self {
        Self::Publish(source)
    }
}

/// Appends an atomic batch of encoded events to a JetStream subject.
///
/// When `expected_last_subject_sequence` is provided, the first publish carries
/// the JetStream subject sequence guard. The final message carries the batch
/// commit marker, and its stream sequence becomes the returned
/// [`StreamPosition`].
pub async fn append_stream<J>(
    js: &J,
    subject: StreamSubject,
    expected_last_subject_sequence: Option<u64>,
    events: &[Event],
) -> Result<StreamPosition, StreamStoreError>
where
    J: JetStreamPublishMessage<PublishError = context::PublishError>,
{
    let batch_id = UuidV7Generator.now_v7().to_string();
    let mut batch_ack = None;

    // Validating every event's headers first surfaces a failure before any
    // batch member reaches the server, without keeping the whole batch's
    // built messages resident.
    let mut validated_headers = Vec::with_capacity(events.len());
    for event in events {
        validated_headers.push(validate_event_headers(event)?);
    }

    for ((index, event), headers) in events.iter().enumerate().zip(validated_headers) {
        let publish = build_publish_message(
            event,
            headers,
            event.content.clone(),
            expected_last_subject_sequence,
            batch_id.as_str(),
            index,
            events.len(),
        );

        let ack = js
            .publish_message(publish.outbound_message(subject.clone()))
            .await
            .map_err(|source| PublishStreamError::PublishEvent { source })?;
        if index == 0 && expected_last_subject_sequence.is_some() && events.len() > 1 {
            acknowledge_batch_member_publish(ack).await?;
        } else if index + 1 == events.len() {
            batch_ack = Some(ack);
        }
    }

    let Some(batch_ack) = batch_ack else {
        return Err(PublishStreamError::MissingBatchCommitAck.into());
    };

    let PublishAck {
        sequence: batch_sequence,
        ..
    } = acknowledge_publish(batch_ack).await?;

    StreamPosition::try_new(batch_sequence).map_err(|source| PublishStreamError::ReadAppendPosition { source }.into())
}

async fn acknowledge_publish<F>(ack: F) -> Result<PublishAck, StreamStoreError>
where
    F: IntoFuture<Output = Result<PublishAck, context::PublishError>>,
{
    match ack.into_future().await {
        Ok(ack @ PublishAck { duplicate: false, .. }) => Ok(ack),
        Ok(PublishAck { duplicate: true, .. }) => Err(PublishStreamError::DuplicateEventId.into()),
        Err(error) if is_wrong_last_sequence(&error) => Err(StreamStoreError::WrongExpectedVersion),
        Err(source) => Err(PublishStreamError::AcknowledgeBatchCommit { source }.into()),
    }
}

async fn acknowledge_batch_member_publish<F>(ack: F) -> Result<(), StreamStoreError>
where
    F: IntoFuture<Output = Result<PublishAck, context::PublishError>>,
{
    match ack.into_future().await {
        Ok(PublishAck { duplicate: false, .. }) => Ok(()),
        Ok(PublishAck { duplicate: true, .. }) => Err(PublishStreamError::DuplicateEventId.into()),
        Err(error) if is_wrong_last_sequence(&error) => Err(StreamStoreError::WrongExpectedVersion),
        Err(error) if is_empty_atomic_batch_member_ack(&error) => Ok(()),
        Err(source) => Err(PublishStreamError::AcknowledgeEvent { source }.into()),
    }
}

fn is_empty_atomic_batch_member_ack(error: &context::PublishError) -> bool {
    error.kind() == PublishErrorKind::Other
        && std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<serde_json::Error>())
            .is_some_and(|source| source.classify() == serde_json::error::Category::Eof)
}

fn is_wrong_last_sequence(error: &context::PublishError) -> bool {
    error.kind() == PublishErrorKind::WrongLastSequence
        || std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<JetStreamError>())
            .is_some_and(|source| {
                matches!(
                    source.error_code(),
                    ErrorCode::STREAM_WRONG_LAST_SEQUENCE | ErrorCode::STREAM_WRONG_LAST_SEQUENCE_CONSTANT
                )
            })
}

fn build_publish_message(
    event: &Event,
    validated_headers: Vec<(HeaderName, HeaderValue)>,
    payload: Vec<u8>,
    expected_last_subject_sequence: Option<u64>,
    batch_id: &str,
    index: usize,
    event_count: usize,
) -> PublishMessage {
    let mut publish = PublishMessage::build()
        .payload(payload.into())
        .message_id(event.id.to_string())
        .header(TROGON_EVENT_TYPE, event.r#type.as_str())
        .header(NATS_BATCH_ID, batch_id)
        .header(NATS_BATCH_SEQUENCE, (index + 1).to_string());
    for (header_name, header_value) in validated_headers {
        publish = publish.header(header_name, header_value);
    }
    if let (0, Some(expected_last_subject_sequence)) = (index, expected_last_subject_sequence) {
        publish = publish.expected_last_subject_sequence(expected_last_subject_sequence);
    }
    if index + 1 == event_count {
        publish = publish.header(NATS_BATCH_COMMIT, "1");
    }
    publish
}

/// Reads all events from the JetStream stream starting at a stream sequence.
pub async fn read_stream(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    let info = stream
        .get_info()
        .await
        .map_err(|source| ReadStreamError::QueryStreamInfo { source })?;
    read_stream_range(stream, from_sequence, info.state.last_sequence).await
}

pub(crate) async fn read_subject_stream(
    stream: &jetstream::stream::Stream,
    stream_id: &str,
    subject: &str,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    let stream_id = stream_id.to_string();
    replay::replay_ordered_range(
        stream,
        Some(subject),
        from_sequence,
        to_sequence,
        replay::ReplayRetryPolicy::default(),
        move |_| stream_id.clone(),
    )
    .await
}

/// Reads the latest stream position for a JetStream subject.
///
/// Missing subjects return `Ok(None)`, which lets resolvers distinguish a new
/// stream from a stream whose current position can be used for optimistic
/// concurrency.
pub async fn subject_current_position<S>(
    stream: &S,
    subject: &StreamSubject,
) -> Result<Option<StreamPosition>, StreamStoreError>
where
    S: JetStreamLastRawMessageBySubject,
{
    match stream.get_last_raw_message_by_subject(subject.as_str()).await {
        Ok(message) => StreamPosition::try_new(message.sequence)
            .map(Some)
            .map_err(|source| ReadStreamError::ReadLatestSubjectPosition { source }.into()),
        Err(error)
            if matches!(
                error.kind(),
                async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(source) => Err(ReadStreamError::ReadLatestSubjectMessage { source }.into()),
    }
}

/// Reads events from an inclusive JetStream stream sequence range.
pub async fn read_stream_range(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    replay::replay_ordered_range(
        stream,
        None,
        from_sequence,
        to_sequence,
        replay::ReplayRetryPolicy::default(),
        |message| message.subject.to_string(),
    )
    .await
}

/// Converts a raw JetStream message into a decider stream event.
///
/// The message must include the NATS message id and Trogon event type headers.
/// User event headers are decoded from [`TROGON_EVENT_HEADER_PREFIX`].
pub fn record_stream_message(
    message: StreamMessage,
    stream_id: impl Into<String>,
) -> Result<StreamEvent, StreamStoreError> {
    let recorded_at = DateTime::<Utc>::from_timestamp(message.time.unix_timestamp(), message.time.nanosecond())
        .ok_or_else(|| ReadStreamError::InvalidMessageTimestamp {
            subject: message.subject.to_string(),
        })?;

    let headers = &message.headers;
    let event_id = required_header(headers, NATS_MESSAGE_ID, "Nats-Msg-Id")?
        .parse::<EventId>()
        .map_err(|source| ReadStreamError::ReadMessageEventId { source })?;
    let event_type = required_header(headers, TROGON_EVENT_TYPE, TROGON_EVENT_TYPE)?.to_string();
    let stream_position =
        StreamPosition::try_new(message.sequence).map_err(|source| ReadStreamError::ReadMessagePosition { source })?;
    let headers = headers_from_nats_headers(headers)?;

    Ok(StreamEvent {
        stream_id: stream_id.into(),
        event: Event {
            id: event_id,
            r#type: event_type,
            content: message.payload.to_vec(),
            headers,
        },
        stream_position,
        recorded_at,
    })
}

fn validate_event_headers(event: &Event) -> Result<Vec<(HeaderName, HeaderValue)>, PublishStreamError> {
    let mut validated = Vec::with_capacity(event.headers.len());
    for (name, value) in event.headers.iter() {
        let header_name = event_header_name(name.as_str())?;
        let header_value = event_header_value(&header_name, value.as_str())?;
        validated.push((header_name, header_value));
    }
    Ok(validated)
}

fn event_header_name(name: &str) -> Result<HeaderName, PublishStreamError> {
    let header_name = format!("{TROGON_EVENT_HEADER_PREFIX}{name}");
    HeaderName::try_from(header_name.as_str())
        .map_err(|source| PublishStreamError::InvalidEventHeaderName { header_name, source })
}

fn event_header_value(header_name: &HeaderName, value: &str) -> Result<HeaderValue, PublishStreamError> {
    value
        .parse::<HeaderValue>()
        .map_err(|source| PublishStreamError::InvalidEventHeaderValue {
            header_name: header_name.to_string(),
            value: value.to_string(),
            source,
        })
}

fn headers_from_nats_headers(headers: &HeaderMap) -> Result<Headers, StreamStoreError> {
    let mut entries = Vec::new();
    for (name, values) in headers.iter() {
        let header_name = name.to_string();
        let Some(event_header_name) = header_name.strip_prefix(TROGON_EVENT_HEADER_PREFIX) else {
            continue;
        };
        let [value] = values.as_slice() else {
            return Err(ReadStreamError::InvalidEventHeaderValueCount { header_name }.into());
        };
        entries.push((event_header_name.to_string(), value.as_str().to_string()));
    }
    Headers::from_entries(entries).map_err(|source| ReadStreamError::ReadMessageEventHeaders { source }.into())
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: impl IntoHeaderName,
    display_name: &'static str,
) -> Result<&'a str, StreamStoreError> {
    headers.get(name).map(|value| value.as_str()).ok_or_else(|| {
        ReadStreamError::MissingMessageHeader {
            header_name: display_name,
        }
        .into()
    })
}

#[cfg(test)]
mod tests;
