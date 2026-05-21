//! Event stream storage helpers backed by JetStream streams.
//!
//! Events are persisted as JetStream messages on caller-resolved subjects.
//! Trogon event metadata is stored in headers so the payload remains the
//! encoded domain event body produced by `trogon-decider-runtime`.

use async_nats::{
    HeaderMap, Subject, SubjectError,
    header::{IntoHeaderName, NATS_MESSAGE_ID},
    jetstream::{
        self, Error as JetStreamError, ErrorCode, context, context::PublishErrorKind, message::PublishMessage,
        publish::PublishAck,
    },
    subject::ToSubject,
};
use chrono::{DateTime, Utc};
use std::{fmt, future::IntoFuture};
use trogon_decider_runtime::{Event, EventId, Headers, StreamEvent, StreamPosition};
use trogon_nats::jetstream::{
    JetStreamGetRawMessage, JetStreamGetStreamInfo, JetStreamLastRawMessageBySubject, JetStreamPublishMessage,
};
use trogon_std::{NowV7, UuidV7Generator};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
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

#[derive(Debug)]
/// Source error with fixed call-site context for stream storage failures.
pub struct StreamStoreSourceError {
    context: &'static str,
    source: BoxError,
}

impl StreamStoreSourceError {
    fn new<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            context,
            source: Box::new(source),
        }
    }

    /// Returns the diagnostic operation context for the failed call.
    pub const fn context(&self) -> &'static str {
        self.context
    }

    /// Returns the typed source error returned by JetStream or envelope decoding.
    pub fn source(&self) -> &(dyn std::error::Error + 'static) {
        self.source.as_ref()
    }
}

impl fmt::Display for StreamStoreSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for StreamStoreSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source())
    }
}

#[derive(Debug)]
/// Error raised while reading or appending stream events.
pub enum StreamStoreError {
    /// JetStream read operation failed.
    Read(StreamStoreSourceError),
    /// JetStream publish operation failed.
    Publish(StreamStoreSourceError),
    /// JetStream rejected the expected last subject sequence.
    WrongExpectedVersion,
}

impl StreamStoreError {
    pub(crate) fn read_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Read(StreamStoreSourceError::new(context, source))
    }

    pub(crate) fn publish_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Publish(StreamStoreSourceError::new(context, source))
    }
}

impl std::fmt::Display for StreamStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(source) => write!(f, "stream read error: {source}"),
            Self::Publish(source) => write!(f, "stream publish error: {source}"),
            Self::WrongExpectedVersion => {
                write!(f, "stream publish error: wrong expected version")
            }
        }
    }
}

impl std::error::Error for StreamStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(source) | Self::Publish(source) => Some(source),
            Self::WrongExpectedVersion => None,
        }
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

    for (index, event) in events.iter().enumerate() {
        let publish = build_publish_message(
            event,
            event.content.clone(),
            expected_last_subject_sequence,
            batch_id.as_str(),
            index,
            events.len(),
        );

        let ack = js
            .publish_message(publish.outbound_message(subject.clone()))
            .await
            .map_err(|source| StreamStoreError::publish_source("failed to publish stream event", source))?;
        if index == 0 && expected_last_subject_sequence.is_some() && events.len() > 1 {
            acknowledge_batch_member_publish(ack, "failed to acknowledge stream event").await?;
        } else if index + 1 == events.len() {
            batch_ack = Some(ack);
        }
    }

    let Some(batch_ack) = batch_ack else {
        return Err(StreamStoreError::publish_source(
            "failed to publish stream event batch",
            std::io::Error::other("batch commit ack was not created"),
        ));
    };

    let PublishAck {
        sequence: batch_sequence,
        ..
    } = acknowledge_publish(batch_ack, "failed to acknowledge stream event batch commit").await?;

    StreamPosition::try_new(batch_sequence)
        .map_err(|source| StreamStoreError::publish_source("failed to read stream append position", source))
}

async fn acknowledge_publish<F>(ack: F, context: &'static str) -> Result<PublishAck, StreamStoreError>
where
    F: IntoFuture<Output = Result<PublishAck, context::PublishError>>,
{
    match ack.into_future().await {
        Ok(ack @ PublishAck { duplicate: false, .. }) => Ok(ack),
        Ok(PublishAck { duplicate: true, .. }) => Err(StreamStoreError::publish_source(
            context,
            std::io::Error::other("duplicate event id"),
        )),
        Err(error) if is_wrong_last_sequence(&error) => Err(StreamStoreError::WrongExpectedVersion),
        Err(error) => Err(StreamStoreError::publish_source(context, error)),
    }
}

async fn acknowledge_batch_member_publish<F>(ack: F, context: &'static str) -> Result<(), StreamStoreError>
where
    F: IntoFuture<Output = Result<PublishAck, context::PublishError>>,
{
    match ack.into_future().await {
        Ok(PublishAck { duplicate: false, .. }) => Ok(()),
        Ok(PublishAck { duplicate: true, .. }) => Err(StreamStoreError::publish_source(
            context,
            std::io::Error::other("duplicate event id"),
        )),
        Err(error) if is_wrong_last_sequence(&error) => Err(StreamStoreError::WrongExpectedVersion),
        Err(error) if is_empty_atomic_batch_member_ack(&error) => Ok(()),
        Err(error) => Err(StreamStoreError::publish_source(context, error)),
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
    for (name, value) in event.headers.iter() {
        publish = publish.header(event_header_name(name.as_str()), value.as_str());
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
pub async fn read_stream<S>(stream: &S, from_sequence: u64) -> Result<Vec<StreamEvent>, StreamStoreError>
where
    S: JetStreamGetStreamInfo + JetStreamGetRawMessage,
{
    let info = stream
        .get_info()
        .await
        .map_err(|source| StreamStoreError::read_source("failed to query stream info", source))?;
    read_stream_range(stream, from_sequence, info.state.last_sequence).await
}

pub(crate) async fn read_subject_stream<S>(
    stream: &S,
    stream_id: &str,
    subject: &str,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError>
where
    S: JetStreamGetRawMessage,
{
    let stream_id = stream_id.to_string();
    read_message_range(
        stream,
        from_sequence,
        to_sequence,
        |message| message.subject.as_str() == subject,
        |_| stream_id.clone(),
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
            .map_err(|source| StreamStoreError::read_source("failed to read latest subject position", source)),
        Err(error)
            if matches!(
                error.kind(),
                async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(StreamStoreError::read_source(
            "failed to read latest subject message",
            error,
        )),
    }
}

/// Reads events from an inclusive JetStream stream sequence range.
pub async fn read_stream_range<S>(
    stream: &S,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError>
where
    S: JetStreamGetRawMessage,
{
    read_message_range(
        stream,
        from_sequence,
        to_sequence,
        |_| true,
        |message| message.subject.to_string(),
    )
    .await
}

async fn read_message_range<S>(
    stream: &S,
    from_sequence: u64,
    to_sequence: u64,
    mut include: impl FnMut(&StreamMessage) -> bool,
    mut stream_id: impl FnMut(&StreamMessage) -> String,
) -> Result<Vec<StreamEvent>, StreamStoreError>
where
    S: JetStreamGetRawMessage,
{
    if from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();
    for sequence in from_sequence..=to_sequence {
        let Some(message) = read_raw_message(stream, sequence).await? else {
            continue;
        };
        if !include(&message) {
            continue;
        }
        let stream_id = stream_id(&message);
        events.push(record_stream_message(message, stream_id)?);
    }

    Ok(events)
}

async fn read_raw_message<S>(stream: &S, sequence: u64) -> Result<Option<StreamMessage>, StreamStoreError>
where
    S: JetStreamGetRawMessage,
{
    match stream.get_raw_message(sequence).await {
        Ok(message) => Ok(Some(message)),
        Err(source)
            if matches!(
                source.kind(),
                async_nats::jetstream::stream::RawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(source) => Err(StreamStoreError::read_source("failed to read stream message", source)),
    }
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
        .ok_or_else(|| {
            StreamStoreError::read_source(
                "failed to convert stream message timestamp into recorded event time",
                std::io::Error::other(message.subject.to_string()),
            )
        })?;

    let headers = &message.headers;
    let event_id = required_header(headers, NATS_MESSAGE_ID, "Nats-Msg-Id")?
        .parse::<EventId>()
        .map_err(|source| StreamStoreError::read_source("failed to read stream message event id", source))?;
    let event_type = required_header(headers, TROGON_EVENT_TYPE, TROGON_EVENT_TYPE)?.to_string();
    let stream_position = StreamPosition::try_new(message.sequence)
        .map_err(|source| StreamStoreError::read_source("failed to read stream message position", source))?;
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

fn event_header_name(name: &str) -> String {
    // TODO: Use `async_nats::HeaderName::try_from` once nats-io/nats.rs#1587
    // lands in a released async-nats version. `HeaderName` intentionally does
    // not duplicate async-nats header-name validation; the adapter should
    // validate the encoded `Trogon-Header-{name}` here with a fallible API
    // instead of relying on `PublishMessage::header`'s panicking conversion.
    format!("{TROGON_EVENT_HEADER_PREFIX}{name}")
}

fn headers_from_nats_headers(headers: &HeaderMap) -> Result<Headers, StreamStoreError> {
    let mut entries = Vec::new();
    for (name, values) in headers.iter() {
        let header_name = name.to_string();
        let Some(event_header_name) = header_name.strip_prefix(TROGON_EVENT_HEADER_PREFIX) else {
            continue;
        };
        let [value] = values.as_slice() else {
            return Err(StreamStoreError::read_source(
                "failed to read stream message event headers",
                std::io::Error::other(format!("event header '{header_name}' must have exactly one value")),
            ));
        };
        entries.push((event_header_name.to_string(), value.as_str().to_string()));
    }
    Headers::from_entries(entries)
        .map_err(|source| StreamStoreError::read_source("failed to read stream message event headers", source))
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: impl IntoHeaderName,
    display_name: &'static str,
) -> Result<&'a str, StreamStoreError> {
    headers.get(name).map(|value| value.as_str()).ok_or_else(|| {
        StreamStoreError::read_source(
            "failed to read stream message event envelope",
            std::io::Error::other(format!("stream message is missing {display_name} header")),
        )
    })
}

#[cfg(test)]
mod tests {
    use async_nats::{
        HeaderMap,
        header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_ID},
        jetstream::{
            context::PublishErrorKind,
            message::StreamMessage,
            stream::{LastRawMessageError, LastRawMessageErrorKind},
        },
    };
    use bytes::Bytes;
    use time::OffsetDateTime;
    use trogon_nats::jetstream::JetStreamGetStream;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumerFactory, MockJetStreamPublishMessage};
    use uuid::Uuid;

    use trogon_decider_runtime::{Event, EventId, Headers, StreamPosition};

    use super::{
        NATS_BATCH_COMMIT, NATS_BATCH_ID, NATS_BATCH_SEQUENCE, StreamStoreError, StreamSubject,
        TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, build_publish_message, headers_from_nats_headers,
        read_stream, read_stream_range, subject_current_position,
    };

    #[test]
    fn stream_subject_accepts_valid_subjects() {
        let subject = StreamSubject::new("cron.jobs.events.backup").unwrap();

        assert_eq!(subject.as_str(), "cron.jobs.events.backup");
        assert_eq!(subject.to_string(), "cron.jobs.events.backup");
    }

    #[test]
    fn stream_subject_rejects_malformed_subjects() {
        for subject in ["", ".events", "events.", "events..backup", "events backup"] {
            assert!(StreamSubject::new(subject).is_err(), "{subject}");
        }
    }

    #[test]
    fn build_publish_message_sets_trogon_event_type_header() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let message = build_publish_message(&event, Vec::new(), Some(0), "batch-1", 0, 1)
            .outbound_message("cron.jobs.events.backup");
        let headers = message.headers.unwrap_or_default();

        assert_eq!(
            headers.get(TROGON_EVENT_TYPE).map(|value| value.as_str()),
            Some("trogon.cron.jobs.v1.JobAdded")
        );
        assert_eq!(
            headers.get(NATS_MESSAGE_ID).map(|value| value.as_str()),
            Some("00000000-0000-0000-0000-000000000001")
        );
    }

    #[test]
    fn build_publish_message_maps_headers_to_trogon_headers() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: Headers::from_entries([("trace-id", "trace-1"), ("tenant", "trogon")]).unwrap(),
        };

        let headers = build_publish_message(&event, Vec::new(), None, "batch-1", 0, 1)
            .outbound_message("cron.jobs.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            headers
                .get(format!("{TROGON_EVENT_HEADER_PREFIX}trace-id").as_str())
                .map(|value| value.as_str()),
            Some("trace-1")
        );
        assert_eq!(
            headers
                .get(format!("{TROGON_EVENT_HEADER_PREFIX}tenant").as_str())
                .map(|value| value.as_str()),
            Some("trogon")
        );
    }

    #[test]
    fn headers_from_nats_headers_reads_trogon_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(format!("{TROGON_EVENT_HEADER_PREFIX}trace-id"), "trace-1");
        headers.insert("Trogon-Event-Type", "test.event");
        headers.insert("Nats-Msg-Id", "00000000-0000-0000-0000-000000000001");

        let parsed_headers = headers_from_nats_headers(&headers).unwrap();

        assert_eq!(
            parsed_headers.get("trace-id").map(|value| value.as_str()),
            Some("trace-1")
        );
        assert_eq!(parsed_headers.len(), 1);
    }

    #[test]
    fn build_publish_message_sets_atomic_batch_occ_on_first_message_only() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let first = build_publish_message(&event, Vec::new(), Some(8), "batch-1", 0, 2)
            .outbound_message("cron.jobs.events.backup")
            .headers
            .unwrap_or_default();
        let second = build_publish_message(&event, Vec::new(), Some(8), "batch-1", 1, 2)
            .outbound_message("cron.jobs.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            first
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            Some("8")
        );
        assert_eq!(
            second
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            None
        );
        assert_eq!(first.get(NATS_BATCH_COMMIT).map(|value| value.as_str()), None);
        assert_eq!(second.get(NATS_BATCH_COMMIT).map(|value| value.as_str()), Some("1"));
    }

    #[test]
    fn build_publish_message_omits_occ_header_without_expected_sequence() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: Headers::empty(),
        };

        let headers = build_publish_message(&event, Vec::new(), None, "batch-1", 0, 1)
            .outbound_message("cron.jobs.events.backup")
            .headers
            .unwrap_or_default();

        assert_eq!(
            headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            None
        );
    }

    #[test]
    fn read_stream_range_rejects_empty_ranges() {
        assert!(read_stream_range_bounds(0, 1));
        assert!(read_stream_range_bounds(1, 0));
        assert!(read_stream_range_bounds(2, 1));
        assert!(!read_stream_range_bounds(1, 1));
    }

    fn read_stream_range_bounds(from_sequence: u64, to_sequence: u64) -> bool {
        from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence
    }

    fn make_event(id: u128, content: &[u8]) -> Event {
        Event {
            id: EventId::from(Uuid::from_u128(id)),
            r#type: "test.event.v1".to_string(),
            content: content.to_vec(),
            headers: Headers::empty(),
        }
    }

    fn make_subject(value: &str) -> StreamSubject {
        StreamSubject::new(value).expect("subject must be valid")
    }

    fn make_stream_message(subject: &str, sequence: u64, event_id: Uuid) -> StreamMessage {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, event_id.to_string().as_str());
        headers.insert(TROGON_EVENT_TYPE, "test.event.v1");
        StreamMessage {
            subject: subject.into(),
            sequence,
            headers,
            payload: Bytes::from_static(b"{}"),
            time: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn make_stream_info(last_sequence: u64) -> async_nats::jetstream::stream::Info {
        serde_json::from_value(serde_json::json!({
            "config": {
                "name": "TEST_STREAM",
                "subjects": [],
                "retention": "limits",
                "max_consumers": -1,
                "max_msgs": -1,
                "max_bytes": -1,
                "discard": "old",
                "max_age": 0,
                "storage": "file",
                "num_replicas": 1
            },
            "created": "1970-01-01T00:00:00Z",
            "state": {
                "messages": 0_u64,
                "bytes": 0_u64,
                "first_seq": 0_u64,
                "first_ts": "1970-01-01T00:00:00Z",
                "last_seq": last_sequence,
                "last_ts": "1970-01-01T00:00:00Z",
                "consumer_count": 0_usize,
                "num_subjects": 0_u64
            },
            "cluster": null,
            "mirror": null,
            "sources": []
        }))
        .expect("test stream info must be valid")
    }

    #[tokio::test]
    async fn append_stream_publishes_single_event_and_returns_position() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_with_sequence(42);

        let position = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"payload")])
            .await
            .expect("publish should succeed");

        assert_eq!(position, StreamPosition::try_new(42).unwrap());
        let messages = js.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "test.events");
        let headers = messages[0].headers.clone().unwrap_or_default();
        assert_eq!(headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), Some("1"));
        assert_eq!(headers.get(NATS_BATCH_SEQUENCE).map(|v| v.as_str()), Some("1"));
    }

    #[tokio::test]
    async fn append_stream_publishes_batch_with_shared_batch_id() {
        let js = MockJetStreamPublishMessage::new();

        let position = append_stream(
            &js,
            make_subject("test.events"),
            Some(0),
            &[make_event(1, b"first"), make_event(2, b"second")],
        )
        .await
        .expect("publish should succeed");

        assert_eq!(position, StreamPosition::try_new(2).unwrap());
        let messages = js.published_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sequence, 1);
        assert_eq!(messages[1].sequence, 2);
        let first_headers = messages[0].headers.clone().unwrap_or_default();
        let second_headers = messages[1].headers.clone().unwrap_or_default();
        let first_batch_id = first_headers.get(NATS_BATCH_ID).map(|v| v.as_str().to_string());
        let second_batch_id = second_headers.get(NATS_BATCH_ID).map(|v| v.as_str().to_string());
        assert!(first_batch_id.is_some());
        assert_eq!(first_batch_id, second_batch_id);
        assert_eq!(
            first_headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|v| v.as_str()),
            Some("0")
        );
        assert_eq!(first_headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), None);
        assert_eq!(second_headers.get(NATS_BATCH_COMMIT).map(|v| v.as_str()), Some("1"));
    }

    #[tokio::test]
    async fn append_stream_uses_semantic_subject_sequence_guard() {
        let js = MockJetStreamPublishMessage::new();

        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(0),
            &[make_event(1, b"first")],
        )
        .await
        .expect("first publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.beta"),
            Some(0),
            &[make_event(2, b"other")],
        )
        .await
        .expect("interleaved publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(3, b"second")],
        )
        .await
        .expect("subject guard should use latest subject sequence");

        let stale = append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(4, b"stale")],
        )
        .await
        .expect_err("stale subject sequence should fail");

        assert!(matches!(stale, StreamStoreError::WrongExpectedVersion));
        assert_eq!(js.last_subject_sequence("test.events.alpha"), 3);
        assert_eq!(js.last_subject_sequence("test.events.beta"), 2);
    }

    #[tokio::test]
    async fn append_stream_maps_wrong_last_sequence_to_optimistic_concurrency() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_error(PublishErrorKind::WrongLastSequence);

        let error = append_stream(&js, make_subject("test.events"), Some(7), &[make_event(1, b"x")])
            .await
            .expect_err("publish should fail");

        assert!(matches!(error, StreamStoreError::WrongExpectedVersion));
    }

    #[tokio::test]
    async fn append_stream_maps_synchronous_publish_error_to_publish_error() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_publish_error(PublishErrorKind::TimedOut);

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("publish should fail");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn append_stream_rejects_duplicate_event_id_via_ack() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_duplicate();

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("duplicate publish should fail");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn append_stream_maps_ack_error_to_publish_error() {
        let js = MockJetStreamPublishMessage::new();
        js.enqueue_ack_error(PublishErrorKind::TimedOut);

        let error = append_stream(&js, make_subject("test.events"), None, &[make_event(1, b"x")])
            .await
            .expect_err("ack error should propagate");

        assert!(matches!(error, StreamStoreError::Publish(_)));
    }

    #[tokio::test]
    async fn read_stream_range_skips_no_message_found_gaps() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events", 1, Uuid::from_u128(1)));
        // sequence 2 missing → mock returns NoMessageFound by default
        factory.add_raw_message(3, make_stream_message("test.events", 3, Uuid::from_u128(3)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = read_stream_range(&stream, 1, 3).await.expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_position, StreamPosition::try_new(1).unwrap());
        assert_eq!(events[1].stream_position, StreamPosition::try_new(3).unwrap());
        assert_eq!(factory.raw_message_calls(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn read_subject_stream_filters_by_subject() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events.a", 1, Uuid::from_u128(1)));
        factory.add_raw_message(2, make_stream_message("test.events.b", 2, Uuid::from_u128(2)));
        factory.add_raw_message(3, make_stream_message("test.events.a", 3, Uuid::from_u128(3)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = super::read_subject_stream(&stream, "test.events.a", "test.events.a", 1, 3)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_id, "test.events.a");
        assert_eq!(events[1].stream_id, "test.events.a");
    }

    #[tokio::test]
    async fn read_subject_stream_uses_caller_stream_id() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message(1, make_stream_message("test.events.a", 1, Uuid::from_u128(1)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = super::read_subject_stream(&stream, "a", "test.events.a", 1, 1)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stream_id, "a");
    }

    #[tokio::test]
    async fn read_subject_stream_observes_semantic_publish_mock() {
        let js = MockJetStreamPublishMessage::new();
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(0),
            &[make_event(1, b"alpha-one")],
        )
        .await
        .expect("alpha publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.beta"),
            Some(0),
            &[make_event(2, b"beta-one")],
        )
        .await
        .expect("beta publish should succeed");
        append_stream(
            &js,
            make_subject("test.events.alpha"),
            Some(1),
            &[make_event(3, b"alpha-two")],
        )
        .await
        .expect("alpha second publish should succeed");

        let events = super::read_subject_stream(&js, "alpha", "test.events.alpha", 1, 3)
            .await
            .expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_id, "alpha");
        assert_eq!(events[0].stream_position, StreamPosition::try_new(1).unwrap());
        assert_eq!(events[1].stream_position, StreamPosition::try_new(3).unwrap());
    }

    #[tokio::test]
    async fn read_stream_range_returns_empty_for_invalid_bounds() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        assert!(read_stream_range(&stream, 0, 1).await.unwrap().is_empty());
        assert!(read_stream_range(&stream, 1, 0).await.unwrap().is_empty());
        assert!(read_stream_range(&stream, 5, 3).await.unwrap().is_empty());
        assert!(factory.raw_message_calls().is_empty());
    }

    #[tokio::test]
    async fn read_stream_range_propagates_non_no_message_errors() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_raw_message_error(1, LastRawMessageErrorKind::Other);
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let error = read_stream_range(&stream, 1, 1)
            .await
            .expect_err("error should propagate");

        assert!(matches!(error, StreamStoreError::Read(_)));
    }

    #[tokio::test]
    async fn read_stream_uses_info_last_sequence_as_upper_bound() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.set_info(make_stream_info(2));
        factory.add_raw_message(1, make_stream_message("test.events", 1, Uuid::from_u128(1)));
        factory.add_raw_message(2, make_stream_message("test.events", 2, Uuid::from_u128(2)));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let events = read_stream(&stream, 1).await.expect("read should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(factory.raw_message_calls(), vec![1, 2]);
    }

    #[tokio::test]
    async fn subject_current_position_returns_none_when_no_message_found() {
        let factory = MockJetStreamConsumerFactory::new();
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, None);
    }

    #[tokio::test]
    async fn subject_current_position_returns_latest_sequence() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message(async_nats::jetstream::message::StreamMessage {
            subject: "test.events.backup".into(),
            sequence: 17,
            headers: async_nats::HeaderMap::new(),
            payload: bytes::Bytes::from_static(b"{}"),
            time: time::OffsetDateTime::UNIX_EPOCH,
        });
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, Some(StreamPosition::try_new(17).unwrap()));
    }

    #[tokio::test]
    async fn subject_current_position_propagates_non_no_message_errors() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));
        let stream = JetStreamGetStream::get_stream(&factory, "TEST_STREAM").await.unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let error = subject_current_position(&stream, &subject).await.unwrap_err();

        assert!(matches!(error, StreamStoreError::Read(_)));
    }
}
