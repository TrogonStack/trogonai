use async_nats::jetstream::message::OutboundMessage;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamPublishMessage, MockJetStreamPublishMessage};

use super::*;

#[tokio::test]
async fn read_rejects_a_stream_message_without_event_headers() {
    let stream = MockJetStreamPublishMessage::new();
    let ack = stream
        .publish_message(OutboundMessage {
            subject: "credential.events.malformed".into(),
            payload: Bytes::from_static(b"payload"),
            headers: None,
        })
        .await
        .unwrap()
        .await
        .unwrap();
    assert_eq!(ack.sequence, 1);

    let error = read_credential_event_stream(&stream, 0).await.unwrap_err();

    assert!(matches!(error, CredentialEventStreamReadError::RecordStreamMessage(_)));
}

#[tokio::test]
async fn read_returns_nothing_when_the_stream_is_empty() {
    let stream = MockJetStreamPublishMessage::new();

    let events = read_credential_event_stream(&stream, 0).await.unwrap();

    assert!(events.is_empty());
}

#[derive(Clone)]
struct FailingRawReads {
    published: MockJetStreamPublishMessage,
    failing_kind: RawMessageErrorKind,
}

impl JetStreamGetStreamInfo for FailingRawReads {
    async fn get_info(&self) -> Result<async_nats::jetstream::stream::Info, InfoError> {
        self.published.get_info().await
    }
}

impl JetStreamGetRawMessage for FailingRawReads {
    async fn get_raw_message(
        &self,
        sequence: u64,
    ) -> Result<async_nats::jetstream::message::StreamMessage, RawMessageError> {
        match sequence {
            1 => Err(RawMessageError::new(RawMessageErrorKind::NoMessageFound)),
            _ => Err(RawMessageError::new(self.failing_kind.clone())),
        }
    }
}

async fn two_published_messages() -> MockJetStreamPublishMessage {
    let published = MockJetStreamPublishMessage::new();
    for subject in ["credential.events.first", "credential.events.second"] {
        published
            .publish_message(OutboundMessage {
                subject: subject.into(),
                payload: Bytes::from_static(b"payload"),
                headers: None,
            })
            .await
            .unwrap()
            .await
            .unwrap();
    }
    published
}

#[tokio::test]
async fn read_surfaces_raw_read_failures_after_skipping_a_deleted_sequence() {
    let stream = FailingRawReads {
        published: two_published_messages().await,
        failing_kind: RawMessageErrorKind::Other,
    };

    let error = read_credential_event_stream(&stream, 0).await.unwrap_err();

    assert!(matches!(
        error,
        CredentialEventStreamReadError::ReadStreamMessage { .. }
    ));
}

#[tokio::test]
async fn read_skips_every_deleted_sequence() {
    let stream = FailingRawReads {
        published: two_published_messages().await,
        failing_kind: RawMessageErrorKind::NoMessageFound,
    };

    let events = read_credential_event_stream(&stream, 0).await.unwrap();

    assert!(events.is_empty());
}
