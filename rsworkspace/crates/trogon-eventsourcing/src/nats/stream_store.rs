use async_nats::{
    HeaderMap,
    header::{IntoHeaderName, NATS_MESSAGE_ID},
    jetstream::{self, context, context::PublishErrorKind, message::PublishMessage, publish::PublishAck},
};
use chrono::{DateTime, Utc};
use trogon_nats::jetstream::JetStreamPublishMessage;
use trogon_std::{NowV7, UuidV7Generator};

use crate::{Event, EventHeaders, EventId, NonEmpty, StreamEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type StreamMessage = async_nats::jetstream::message::StreamMessage;

const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
const NATS_BATCH_ID: &str = "Nats-Batch-Id";
const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";
pub const TROGON_EVENT_HEADER_PREFIX: &str = "Trogon-Header-";
pub const TROGON_EVENT_TYPE: &str = "Trogon-Event-Type";

#[derive(Debug)]
pub enum StreamStoreError {
    Read { context: &'static str, source: BoxError },
    Publish { context: &'static str, source: BoxError },
    WrongExpectedVersion,
}

impl StreamStoreError {
    pub(crate) fn read_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Read {
            context,
            source: Box::new(source),
        }
    }

    pub(crate) fn publish_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Publish {
            context,
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for StreamStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { context, source } => write!(f, "stream read error: {context}: {source}"),
            Self::Publish { context, source } => {
                write!(f, "stream publish error: {context}: {source}")
            }
            Self::WrongExpectedVersion => {
                write!(f, "stream publish error: wrong expected version")
            }
        }
    }
}

impl std::error::Error for StreamStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Publish { source, .. } => Some(source.as_ref()),
            Self::WrongExpectedVersion => None,
        }
    }
}

pub async fn append_stream<J>(
    js: &J,
    subject: String,
    expected_last_subject_sequence: Option<u64>,
    events: &NonEmpty<Event>,
) -> Result<crate::StreamPosition, StreamStoreError>
where
    J: JetStreamPublishMessage<PublishError = context::PublishError, AckFuture = context::PublishAckFuture>,
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
        if index + 1 == events.len() {
            batch_ack = Some(ack);
        }
    }

    let Some(batch_ack) = batch_ack else {
        return Err(StreamStoreError::publish_source(
            "failed to publish stream event batch",
            std::io::Error::other("batch commit ack was not created"),
        ));
    };

    let batch_sequence = match batch_ack.into_future().await {
        Ok(PublishAck {
            sequence,
            duplicate: false,
            ..
        }) => sequence,
        Ok(PublishAck { duplicate: true, .. }) => {
            return Err(StreamStoreError::publish_source(
                "failed to acknowledge stream event batch commit",
                std::io::Error::other("duplicate event id"),
            ));
        }
        Err(error) if error.kind() == PublishErrorKind::WrongLastSequence => {
            return Err(StreamStoreError::WrongExpectedVersion);
        }
        Err(error) => {
            return Err(StreamStoreError::publish_source(
                "failed to acknowledge stream event batch commit",
                error,
            ));
        }
    };

    crate::StreamPosition::try_new(batch_sequence)
        .map_err(|source| StreamStoreError::publish_source("failed to read stream append position", source))
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
        publish = publish.header(event_header_name(name.as_str()), value);
    }
    if let (0, Some(expected_last_subject_sequence)) = (index, expected_last_subject_sequence) {
        publish = publish.expected_last_subject_sequence(expected_last_subject_sequence);
    }
    if index + 1 == event_count {
        publish = publish.header(NATS_BATCH_COMMIT, "1");
    }
    publish
}

pub async fn read_stream(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    let info = stream
        .get_info()
        .await
        .map_err(|source| StreamStoreError::read_source("failed to query stream info", source))?;
    read_stream_range(stream, from_sequence, info.state.last_sequence).await
}

pub(crate) async fn read_subject_stream(
    stream: &jetstream::stream::Stream,
    subject: &str,
    from_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    let info = stream
        .get_info()
        .await
        .map_err(|source| StreamStoreError::read_source("failed to query stream info", source))?;
    read_subject_range(stream, subject, from_sequence, info.state.last_sequence).await
}

pub async fn read_stream_range(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    read_message_range(stream, from_sequence, to_sequence, |_| true).await
}

async fn read_subject_range(
    stream: &jetstream::stream::Stream,
    subject: &str,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
    read_message_range(stream, from_sequence, to_sequence, |message| {
        message.subject.as_str() == subject
    })
    .await
}

async fn read_message_range(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
    to_sequence: u64,
    mut include: impl FnMut(&StreamMessage) -> bool,
) -> Result<Vec<StreamEvent>, StreamStoreError> {
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
        events.push(record_stream_message(message)?);
    }

    Ok(events)
}

async fn read_raw_message(
    stream: &jetstream::stream::Stream,
    sequence: u64,
) -> Result<Option<StreamMessage>, StreamStoreError> {
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

pub fn record_stream_message(message: StreamMessage) -> Result<StreamEvent, StreamStoreError> {
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
    let subject = message.subject.to_string();
    let stream_position = crate::StreamPosition::try_new(message.sequence)
        .map_err(|source| StreamStoreError::read_source("failed to read stream message position", source))?;
    let event_headers = event_headers_from_headers(headers)?;

    Ok(StreamEvent {
        stream_id: subject,
        event: Event {
            id: event_id,
            r#type: event_type,
            content: message.payload.to_vec(),
            headers: event_headers,
        },
        stream_position,
        recorded_at,
    })
}

fn event_header_name(name: &str) -> String {
    format!("{TROGON_EVENT_HEADER_PREFIX}{name}")
}

fn event_headers_from_headers(headers: &HeaderMap) -> Result<EventHeaders, StreamStoreError> {
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
    EventHeaders::from_entries(entries)
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
    };
    use uuid::Uuid;

    use crate::{Event, EventHeaders, EventId};

    use super::{
        NATS_BATCH_COMMIT, TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, build_publish_message,
        event_headers_from_headers,
    };

    #[test]
    fn build_publish_message_sets_trogon_event_type_header() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: EventHeaders::empty(),
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
    fn build_publish_message_maps_event_headers_to_trogon_headers() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: EventHeaders::from_entries([("trace-id", "trace-1"), ("tenant", "trogon")]).unwrap(),
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
    fn event_headers_from_headers_reads_trogon_event_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(format!("{TROGON_EVENT_HEADER_PREFIX}trace-id"), "trace-1");
        headers.insert("Trogon-Event-Type", "test.event");
        headers.insert("Nats-Msg-Id", "00000000-0000-0000-0000-000000000001");

        let event_headers = event_headers_from_headers(&headers).unwrap();

        assert_eq!(event_headers.get("trace-id"), Some("trace-1"));
        assert_eq!(event_headers.len(), 1);
    }

    #[test]
    fn build_publish_message_sets_atomic_batch_occ_on_first_message_only() {
        let event = Event {
            id: EventId::from(Uuid::from_u128(1)),
            r#type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            content: Vec::new(),
            headers: EventHeaders::empty(),
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
            headers: EventHeaders::empty(),
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
}
