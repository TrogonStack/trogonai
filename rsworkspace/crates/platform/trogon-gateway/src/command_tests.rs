use super::*;
use async_nats::jetstream::message::StreamMessage;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamEvent, StreamRead};
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::{MockJetStreamConsumerFactory, MockJetStreamKvStore, MockJetStreamPublishMessage};

#[derive(Clone, Default)]
struct EmptyCredentialEventStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("empty credential event store failed")]
struct EmptyCredentialEventStoreError;

impl StreamRead<str> for EmptyCredentialEventStore {
    type Error = EmptyCredentialEventStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().unwrap();
        Ok(ReadStreamResponse {
            current_position: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id)
                .map(|event| event.stream_position)
                .max(),
            events: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                .cloned()
                .collect(),
        })
    }
}

#[tokio::test]
async fn credential_runtime_projection_refresh_handles_empty_stream() {
    let stream = MockJetStreamPublishMessage::new();
    let event_store = EmptyCredentialEventStore::default();
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let checkpoints = credential::processor::runtime_projection::RuntimeProjectionKvCheckpointStore::new(kv);
    let registry = credential::processor::runtime_projection::RuntimeCredentialRegistry::default();

    let (_registry, report) =
        refresh_credential_runtime_projection_with_checkpoints(&stream, &event_store, &checkpoints, registry)
            .await
            .unwrap();

    assert_eq!(report.scanned_events(), 0);
    assert_eq!(report.decoded_events(), 0);
    assert_eq!(report.projected_integrations(), 0);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_advanced_to(), None);
}

#[tokio::test]
async fn notion_verification_token_command_reads_latest_message() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    js.add_last_raw_message(stream_message(
        "notion-primary.subscription.verification",
        Bytes::from_static(br#"{"verification_token":"secret_token"}"#),
    ));
    let mut out = Vec::new();

    notion_verification_token(&resolved, &integration("primary"), false, &nats, &js, &mut out)
        .await
        .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "secret_token\n");
    assert!(nats.subscribed_to().is_empty());
    assert_eq!(js.get_stream_calls(), vec!["NOTION_PRIMARY"]);
    assert_eq!(
        js.last_raw_message_subjects(),
        vec!["notion-primary.subscription.verification"]
    );
}

#[tokio::test]
async fn notion_verification_token_command_watches_subscription() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let messages = nats.inject_messages();
    messages
        .unbounded_send(nats_message(
            "notion-primary.subscription.verification",
            br#"{"verification_token":"watched_token"}"#,
        ))
        .unwrap();
    drop(messages);
    let js = MockJetStreamConsumerFactory::new();
    let mut out = Vec::new();

    notion_verification_token(&resolved, &integration("primary"), true, &nats, &js, &mut out)
        .await
        .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "watched_token\n");
    assert_eq!(nats.subscribed_to(), vec!["notion-primary.subscription.verification"]);
    assert!(js.get_stream_calls().is_empty());
}

#[tokio::test]
async fn notion_verification_token_command_rejects_unknown_integration_before_using_deps() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    let mut out = Vec::new();

    let error = notion_verification_token(&resolved, &integration("missing"), false, &nats, &js, &mut out)
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "notion integration 'missing' is not configured");
    assert!(error.downcast_ref::<NotionVerificationTokenCommandError>().is_some());
    assert!(out.is_empty());
    assert!(nats.subscribed_to().is_empty());
    assert!(js.get_stream_calls().is_empty());
}

fn resolved_config() -> config::ResolvedConfig {
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    write!(
        file,
        r#"
[sources.notion.integrations.primary.webhook]
verification_token = "configured-token"
"#
    )
    .unwrap();

    config::load(Some(file.path())).unwrap()
}

fn integration(value: &str) -> source_integration_id::SourceIntegrationId {
    source_integration_id::SourceIntegrationId::new(value).unwrap()
}

fn nats_message(subject: &str, payload: &'static [u8]) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::from_static(payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

fn stream_message(subject: &str, payload: Bytes) -> StreamMessage {
    StreamMessage {
        subject: subject.into(),
        sequence: 1,
        headers: async_nats::HeaderMap::new(),
        payload,
        time: OffsetDateTime::UNIX_EPOCH,
    }
}
