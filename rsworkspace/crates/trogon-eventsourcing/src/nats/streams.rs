use async_nats::{
    HeaderMap, HeaderName,
    header::NATS_MESSAGE_ID,
    jetstream::{self, context, context::PublishErrorKind, message::PublishMessage, publish::PublishAck},
};
use chrono::{DateTime, Utc};
use trogon_nats::jetstream::JetStreamPublishMessage;
use trogon_std::{NowV7, UuidV7Generator};

use crate::{EventData, EventId, NonEmpty, RecordedEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
const NATS_BATCH_ID: &str = "Nats-Batch-Id";
const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";
pub const TROGON_EVENT_TYPE: &str = "Trogon-Event-Type";

#[derive(Debug)]
pub enum StreamStoreError {
    Read { context: &'static str, source: BoxError },
    Publish { context: &'static str, source: BoxError },
    WrongExpectedVersion,
}

impl StreamStoreError {
    fn read_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Read {
            context,
            source: Box::new(source),
        }
    }

    fn publish_source<E>(context: &'static str, source: E) -> Self
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
    expected_last_subject_sequence: u64,
    events: &NonEmpty<EventData>,
) -> Result<(), StreamStoreError>
where
    J: JetStreamPublishMessage<PublishError = context::PublishError, AckFuture = context::PublishAckFuture>,
{
    append_stream_with_uuid_generator(js, subject, expected_last_subject_sequence, events, &UuidV7Generator).await
}

async fn append_stream_with_uuid_generator<J, N>(
    js: &J,
    subject: String,
    expected_last_subject_sequence: u64,
    events: &NonEmpty<EventData>,
    now_v7: &N,
) -> Result<(), StreamStoreError>
where
    J: JetStreamPublishMessage<PublishError = context::PublishError, AckFuture = context::PublishAckFuture>,
    N: NowV7,
{
    let first_stream_id = events.first().stream_id().to_string();
    if events.iter().any(|event| event.stream_id() != first_stream_id) {
        return Err(StreamStoreError::publish_source(
            "failed to publish stream event batch",
            std::io::Error::other("batch contains events across multiple streams"),
        ));
    }
    if events.iter().any(|event| event.metadata.is_some()) {
        return Err(StreamStoreError::publish_source(
            "failed to publish stream event batch",
            std::io::Error::other("event metadata is not supported by the JetStream stream store"),
        ));
    }

    let batch_id = now_v7.now_v7().to_string();
    let mut ack_futures = Vec::with_capacity(events.len());

    for (index, event) in events.iter().enumerate() {
        let publish = build_publish_message(
            event,
            event.payload.clone(),
            expected_last_subject_sequence,
            batch_id.as_str(),
            index,
            events.len(),
        );

        let ack = js
            .publish_message(publish.outbound_message(subject.clone()))
            .await
            .map_err(|source| StreamStoreError::publish_source("failed to publish stream event", source))?;
        ack_futures.push(ack);
    }

    for ack in ack_futures {
        match ack.into_future().await {
            Ok(PublishAck { .. }) => {}
            Err(error) if error.kind() == PublishErrorKind::WrongLastSequence => {
                return Err(StreamStoreError::WrongExpectedVersion);
            }
            Err(error) => {
                return Err(StreamStoreError::publish_source(
                    "failed to acknowledge stream event batch",
                    error,
                ));
            }
        }
    }

    Ok(())
}

fn build_publish_message(
    event: &EventData,
    payload: Vec<u8>,
    expected_last_subject_sequence: u64,
    batch_id: &str,
    index: usize,
    event_count: usize,
) -> PublishMessage {
    let mut publish = PublishMessage::build()
        .payload(payload.into())
        .message_id(event.event_id.to_string())
        .expected_last_subject_sequence(expected_last_subject_sequence + index as u64)
        .header(TROGON_EVENT_TYPE, event.event_type.as_str())
        .header(NATS_BATCH_ID, batch_id)
        .header(NATS_BATCH_SEQUENCE, (index + 1).to_string());
    if index + 1 == event_count {
        publish = publish.header(NATS_BATCH_COMMIT, "1");
    }
    publish
}

pub async fn read_stream_from(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
) -> Result<Vec<RecordedEvent>, StreamStoreError> {
    let info = stream
        .get_info()
        .await
        .map_err(|source| StreamStoreError::read_source("failed to query stream info", source))?;
    read_stream_range(stream, from_sequence, info.state.last_sequence).await
}

pub async fn read_stream_range(
    stream: &jetstream::stream::Stream,
    from_sequence: u64,
    to_sequence: u64,
) -> Result<Vec<RecordedEvent>, StreamStoreError> {
    if from_sequence == 0 || to_sequence == 0 || from_sequence > to_sequence {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();
    for sequence in from_sequence..=to_sequence {
        let Some(message) = read_raw_message(stream, sequence).await? else {
            continue;
        };
        events.push(record_stream_message(message)?);
    }

    Ok(events)
}

async fn read_raw_message(
    stream: &jetstream::stream::Stream,
    sequence: u64,
) -> Result<Option<async_nats::jetstream::message::StreamMessage>, StreamStoreError> {
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

pub fn record_stream_message(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<RecordedEvent, StreamStoreError> {
    let recorded_at = DateTime::<Utc>::from_timestamp(message.time.unix_timestamp(), message.time.nanosecond())
        .ok_or_else(|| {
            StreamStoreError::read_source(
                "failed to convert stream message timestamp into recorded event time",
                std::io::Error::other(message.subject.to_string()),
            )
        })?;

    let headers = &message.headers;
    let event_id = required_header_name(headers, NATS_MESSAGE_ID, "Nats-Msg-Id")?
        .parse::<EventId>()
        .map_err(|source| StreamStoreError::read_source("failed to read stream message event id", source))?;
    let event_type = required_header_str(headers, TROGON_EVENT_TYPE, TROGON_EVENT_TYPE)?.to_string();
    let subject = message.subject.to_string();

    Ok(RecordedEvent::new(
        event_id,
        event_type,
        subject.clone(),
        message.payload.to_vec(),
        None,
        subject,
        None,
        Some(message.sequence),
        recorded_at,
    ))
}

fn required_header_name<'a>(
    headers: &'a HeaderMap,
    name: HeaderName,
    display_name: &'static str,
) -> Result<&'a str, StreamStoreError> {
    headers.get(name).map(|value| value.as_str()).ok_or_else(|| {
        StreamStoreError::read_source(
            "failed to read stream message event envelope",
            std::io::Error::other(format!("stream message is missing {display_name} header")),
        )
    })
}

fn required_header_str<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
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
    use async_nats::header::NATS_MESSAGE_ID;
    use uuid::Uuid;

    use crate::{EventData, EventId};

    use super::{TROGON_EVENT_TYPE, build_publish_message};

    #[test]
    fn build_publish_message_sets_trogon_event_type_header() {
        let event = EventData {
            event_id: EventId::from(Uuid::from_u128(1)),
            event_type: "trogon.cron.jobs.v1.JobAdded".to_string(),
            stream_id: "backup".to_string(),
            payload: Vec::new(),
            metadata: None,
        };

        let message =
            build_publish_message(&event, Vec::new(), 0, "batch-1", 0, 1).outbound_message("cron.jobs.events.backup");
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
