//! Message publishing and subscription helpers

use async_nats::Client;
use futures::StreamExt;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, error, trace, warn};

use crate::error::{Error, Result};

/// Message publisher for sending events and commands.
///
/// When a JetStream context is provided (via [`MessagePublisher::with_jetstream`]),
/// [`publish_command`] uses `js.publish_with_headers()` and awaits the `PubAck`
/// to confirm the message was durably stored in the stream before returning.
/// Without a JetStream context it falls back to a plain core-NATS publish.
#[derive(Clone)]
pub struct MessagePublisher {
    client: Client,
    js: Option<async_nats::jetstream::Context>,
    prefix: String,
}

impl MessagePublisher {
    /// Create a new message publisher (plain NATS publish, no PubAck).
    pub fn new(client: Client, prefix: impl Into<String>) -> Self {
        Self {
            client,
            js: None,
            prefix: prefix.into(),
        }
    }

    /// Create a publisher backed by a JetStream context.
    ///
    /// [`publish_command`] will use `js.publish_with_headers()` and await the
    /// `PubAck`, guaranteeing the command was persisted in the stream before
    /// the call returns.  All other `publish*` helpers continue to use the
    /// plain NATS client (fire-and-forget, adequate for ephemeral events).
    pub fn with_jetstream(
        client: Client,
        js: async_nats::jetstream::Context,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            client,
            js: Some(js),
            prefix: prefix.into(),
        }
    }

    /// Get the prefix
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Publish a message to a subject
    pub async fn publish<T: Serialize>(&self, subject: impl AsRef<str>, message: &T) -> Result<()> {
        let subject = subject.as_ref();
        let payload = serde_json::to_vec(message).map_err(Error::Serialization)?;

        trace!(
            "Publishing to subject: {}, size: {} bytes",
            subject,
            payload.len()
        );

        self.client
            .publish(subject.to_string(), payload.into())
            .await
            .map_err(|e| Error::Publish(format!("Failed to publish to {}: {}", subject, e)))?;

        debug!("Published message to {}", subject);
        Ok(())
    }

    /// Publish with custom headers
    pub async fn publish_with_headers<T: Serialize>(
        &self,
        subject: impl AsRef<str>,
        message: &T,
        headers: async_nats::HeaderMap,
    ) -> Result<()> {
        let subject = subject.as_ref();
        let payload = serde_json::to_vec(message).map_err(Error::Serialization)?;

        trace!("Publishing to subject: {} with headers", subject);

        self.client
            .publish_with_headers(subject.to_string(), headers, payload.into())
            .await
            .map_err(|e| Error::Publish(format!("Failed to publish to {}: {}", subject, e)))?;

        debug!("Published message with headers to {}", subject);
        Ok(())
    }

    /// Publish a command with tracing metadata carried as NATS headers.
    ///
    /// The `CommandMetadata` is serialised into well-known headers (`X-Command-Id`,
    /// `X-Session-Id`, `X-Produced-At`, `X-Causation-Id`, `X-Correlation-Id`,
    /// `X-Attempt`) so every consumer can extract it without touching the
    /// command payload struct.
    ///
    /// When this publisher was created with [`with_jetstream`], the publish goes
    /// through the JetStream context and the call blocks until a `PubAck` is
    /// received from the server, guaranteeing the command was durably stored in
    /// the stream.  Otherwise it falls back to a plain core-NATS publish.
    pub async fn publish_command<T: Serialize>(
        &self,
        subject: impl AsRef<str>,
        command: &T,
        metadata: telegram_types::commands::CommandMetadata,
    ) -> Result<()> {
        let subject = subject.as_ref();

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("X-Command-Id", metadata.command_id.to_string().as_str());
        headers.insert("X-Produced-At", metadata.produced_at.to_rfc3339().as_str());
        headers.insert("X-Attempt", metadata.attempt.to_string().as_str());
        if let Some(ref sid) = metadata.session_id {
            headers.insert("X-Session-Id", sid.as_str());
        }
        if let Some(ref cid) = metadata.causation_id {
            headers.insert("X-Causation-Id", cid.as_str());
        }
        if let Some(ref rid) = metadata.correlation_id {
            headers.insert("X-Correlation-Id", rid.as_str());
        }

        let payload = serde_json::to_vec(command).map_err(Error::Serialization)?;

        if let Some(ref js) = self.js {
            // JetStream publish: await PubAck to confirm durable persistence.
            trace!("JetStream publish_command to subject: {}", subject);
            let ack_future = js
                .publish_with_headers(subject.to_string(), headers, payload.into())
                .await
                .map_err(|e| {
                    Error::Publish(format!("JetStream publish to {} failed: {}", subject, e))
                })?;
            ack_future.await.map_err(|e| {
                Error::Publish(format!("JetStream PubAck for {} failed: {}", subject, e))
            })?;
            debug!("JetStream publish_command acked on {}", subject);
        } else {
            // Plain NATS fallback (fire-and-forget, no PubAck).
            warn!(
                "publish_command on '{}' using plain NATS (no JetStream context — PubAck unavailable)",
                subject
            );
            self.client
                .publish_with_headers(subject.to_string(), headers, payload.into())
                .await
                .map_err(|e| Error::Publish(format!("Failed to publish to {}: {}", subject, e)))?;
            debug!("Published command (plain NATS) to {}", subject);
        }

        Ok(())
    }
}

/// Read `CommandMetadata` fields from NATS message headers.
///
/// Returns `None` if `X-Command-Id` is absent or unparseable.
pub fn read_cmd_metadata(
    headers: &async_nats::HeaderMap,
) -> Option<telegram_types::commands::CommandMetadata> {
    use std::str::FromStr;
    let command_id = uuid::Uuid::from_str(headers.get("X-Command-Id")?.as_str()).ok()?;

    let produced_at = headers
        .get("X-Produced-At")
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v.as_str()).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    let attempt = headers
        .get("X-Attempt")
        .and_then(|v| v.as_str().parse::<u32>().ok())
        .unwrap_or(1);

    Some(telegram_types::commands::CommandMetadata {
        command_id,
        session_id: headers.get("X-Session-Id").map(|v| v.as_str().to_string()),
        produced_at,
        causation_id: headers
            .get("X-Causation-Id")
            .map(|v| v.as_str().to_string()),
        correlation_id: headers
            .get("X-Correlation-Id")
            .map(|v| v.as_str().to_string()),
        attempt,
    })
}

/// Message subscriber for receiving events and commands
pub struct MessageSubscriber {
    client: Client,
    prefix: String,
}

impl MessageSubscriber {
    /// Create a new message subscriber
    pub fn new(client: Client, prefix: impl Into<String>) -> Self {
        Self {
            client,
            prefix: prefix.into(),
        }
    }

    /// Get the prefix
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Get a reference to the NATS client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Subscribe to a subject and deserialize messages
    pub async fn subscribe<T: DeserializeOwned>(
        &self,
        subject: impl AsRef<str>,
    ) -> Result<MessageStream<T>> {
        let subject = subject.as_ref();
        debug!("Subscribing to subject: {}", subject);

        let subscriber = self
            .client
            .subscribe(subject.to_string())
            .await
            .map_err(|e| Error::Subscribe(format!("Failed to subscribe to {}: {}", subject, e)))?;

        Ok(MessageStream {
            subscriber,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Subscribe to a subject with queue group
    pub async fn queue_subscribe<T: DeserializeOwned>(
        &self,
        subject: impl AsRef<str>,
        queue_group: impl Into<String>,
    ) -> Result<MessageStream<T>> {
        let subject = subject.as_ref();
        let queue_group = queue_group.into();
        debug!(
            "Queue subscribing to subject: {} (group: {})",
            subject, queue_group
        );

        let subscriber = self
            .client
            .queue_subscribe(subject.to_string(), queue_group)
            .await
            .map_err(|e| {
                Error::Subscribe(format!("Failed to queue subscribe to {}: {}", subject, e))
            })?;

        Ok(MessageStream {
            subscriber,
            _phantom: std::marker::PhantomData,
        })
    }
}

/// Stream of deserialized messages
pub struct MessageStream<T> {
    subscriber: async_nats::Subscriber,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: DeserializeOwned> MessageStream<T> {
    /// Get the next message from the stream
    pub async fn next(&mut self) -> Option<Result<T>> {
        match self.subscriber.next().await {
            Some(msg) => {
                trace!("Received message on subject: {}", msg.subject);

                match serde_json::from_slice(&msg.payload) {
                    Ok(data) => Some(Ok(data)),
                    Err(e) => {
                        error!("Failed to deserialize message: {}", e);
                        Some(Err(Error::Serialization(e)))
                    }
                }
            }
            None => None,
        }
    }

    /// Get the next raw message without deserialization
    pub async fn next_raw(&mut self) -> Option<async_nats::Message> {
        self.subscriber.next().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn test_message_publisher_creation() {
        // Verifies the types compile correctly
    }

    // ── NATS integration ──────────────────────────────────────────────────────

    const NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<Client> {
        async_nats::connect(NATS_URL).await.ok()
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestMsg {
        value: String,
        count: u32,
    }

    #[tokio::test]
    async fn test_publish_subscribe_roundtrip() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.roundtrip.{}", uuid::Uuid::new_v4().simple());

        let publisher = MessagePublisher::new(client.clone(), "test");
        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

        let sent = TestMsg {
            value: "hello".to_string(),
            count: 42,
        };
        publisher.publish(&subject, &sent).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received, sent);
    }

    #[tokio::test]
    async fn test_publish_with_headers_roundtrip() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.headers.{}", uuid::Uuid::new_v4().simple());

        let publisher = MessagePublisher::new(client.clone(), "test");
        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("x-source", "test");

        let sent = TestMsg {
            value: "with-headers".to_string(),
            count: 1,
        };
        publisher
            .publish_with_headers(&subject, &sent, headers)
            .await
            .unwrap();

        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received, sent);
    }

    #[tokio::test]
    async fn test_queue_subscribe_roundtrip() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.queue.{}", uuid::Uuid::new_v4().simple());

        let publisher = MessagePublisher::new(client.clone(), "test");
        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber
            .queue_subscribe::<TestMsg>(&subject, "workers")
            .await
            .unwrap();

        let sent = TestMsg {
            value: "queued".to_string(),
            count: 7,
        };
        publisher.publish(&subject, &sent).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received, sent);
    }

    #[tokio::test]
    async fn test_deserialize_error_on_invalid_json() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.invalid.{}", uuid::Uuid::new_v4().simple());

        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

        // Publish raw invalid JSON
        client
            .publish(subject, b"not-valid-json".as_ref().into())
            .await
            .unwrap();

        let result = stream.next().await.unwrap();
        assert!(result.is_err(), "invalid JSON must return an error");
    }

    #[tokio::test]
    async fn test_next_raw_receives_bytes() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.raw.{}", uuid::Uuid::new_v4().simple());

        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

        client
            .publish(subject, b"{\"value\":\"raw\",\"count\":0}".as_ref().into())
            .await
            .unwrap();

        let raw = stream.next_raw().await.unwrap();
        assert_eq!(&raw.payload[..], b"{\"value\":\"raw\",\"count\":0}");
    }

    #[tokio::test]
    async fn test_publisher_prefix_accessor() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let publisher = MessagePublisher::new(client, "mypfx");
        assert_eq!(publisher.prefix(), "mypfx");
    }

    #[tokio::test]
    async fn test_subscriber_prefix_and_client_accessors() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subscriber = MessageSubscriber::new(client, "mypfx");
        assert_eq!(subscriber.prefix(), "mypfx");
        // client() must return a usable client
        let _ = subscriber.client();
    }

    // ── Issue 4: CommandMetadata headers roundtrip ────────────────────────────

    #[test]
    fn test_read_cmd_metadata_missing_command_id_returns_none() {
        // X-Command-Id is required — absent means no metadata
        let headers = async_nats::HeaderMap::new();
        assert!(
            read_cmd_metadata(&headers).is_none(),
            "absent X-Command-Id must yield None"
        );
    }

    #[test]
    fn test_read_cmd_metadata_only_command_id_fills_defaults() {
        // Partial headers: only X-Command-Id → attempt defaults to 1, optionals are None
        let mut headers = async_nats::HeaderMap::new();
        let id = uuid::Uuid::new_v4();
        headers.insert("X-Command-Id", id.to_string().as_str());

        let meta = read_cmd_metadata(&headers).expect("must parse with just X-Command-Id");
        assert_eq!(meta.command_id, id);
        assert_eq!(meta.attempt, 1, "attempt must default to 1");
        assert!(meta.session_id.is_none());
        assert!(meta.causation_id.is_none());
        assert!(meta.correlation_id.is_none());
    }

    #[tokio::test]
    async fn test_publish_command_sets_all_metadata_headers() {
        // Verifies that publish_command serialises every CommandMetadata field
        // into NATS headers and read_cmd_metadata recovers them correctly.
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };

        let subject = format!("test.metadata.headers.{}", uuid::Uuid::new_v4().simple());

        let meta = telegram_types::commands::CommandMetadata::new()
            .with_causation("session-abc", "event-xyz");
        let expected_cmd_id = meta.command_id;
        let expected_session = meta.session_id.clone().unwrap();
        let expected_causation = meta.causation_id.clone().unwrap();
        let expected_attempt = meta.attempt;

        let publisher = MessagePublisher::new(client.clone(), "test");
        let mut raw_sub = client.subscribe(subject.clone()).await.unwrap();

        publisher
            .publish_command(
                &subject,
                &TestMsg {
                    value: "cmd".into(),
                    count: 0,
                },
                meta,
            )
            .await
            .unwrap();

        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut raw_sub),
        )
        .await
        .expect("timed out waiting for message")
        .expect("subscriber closed");

        let headers = msg.headers.as_ref().expect("headers must be present");
        let read_back =
            read_cmd_metadata(headers).expect("must parse CommandMetadata from headers");

        assert_eq!(
            read_back.command_id, expected_cmd_id,
            "command_id must round-trip"
        );
        assert_eq!(
            read_back.session_id.as_deref(),
            Some(expected_session.as_str()),
            "session_id must round-trip"
        );
        assert_eq!(
            read_back.causation_id.as_deref(),
            Some(expected_causation.as_str()),
            "causation_id must round-trip"
        );
        assert_eq!(
            read_back.attempt, expected_attempt,
            "attempt must round-trip"
        );
    }

    #[tokio::test]
    async fn test_multiple_messages_in_order() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.messaging.order.{}", uuid::Uuid::new_v4().simple());

        let publisher = MessagePublisher::new(client.clone(), "test");
        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

        for i in 0..3u32 {
            publisher
                .publish(
                    &subject,
                    &TestMsg {
                        value: "msg".to_string(),
                        count: i,
                    },
                )
                .await
                .unwrap();
        }

        for i in 0..3u32 {
            let msg = stream.next().await.unwrap().unwrap();
            assert_eq!(msg.count, i);
        }
    }

    // ── Issue: outbound at-least-once + PubAck ───────────────────────────────

    /// `publish_command` with a JetStream context must await a PubAck and store
    /// the message durably in the `telegram_commands_{prefix}` stream.
    ///
    /// This verifies the fix: `MessagePublisher::with_jetstream().publish_command()`
    /// uses `js.publish_with_headers()` instead of plain `client.publish_with_headers()`.
    #[tokio::test]
    async fn test_publish_command_jetstream_persists_in_stream() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("cmdstream-{}", uuid::Uuid::new_v4().simple());

        // Create the outbound command stream (agent startup would do this)
        crate::nats::setup_agent_stream(&js, &prefix).await.unwrap();

        let publisher = MessagePublisher::with_jetstream(client.clone(), js.clone(), &prefix);
        let subject = crate::subjects::agent::message_send(&prefix);

        let meta = telegram_types::commands::CommandMetadata::new()
            .with_causation("session:42", "event:abc");
        let expected_cmd_id = meta.command_id;

        // publish_command must block until PubAck is received
        publisher
            .publish_command(
                &subject,
                &TestMsg {
                    value: "outbound".into(),
                    count: 1,
                },
                meta,
            )
            .await
            .expect("publish_command with JetStream must succeed and return PubAck");

        // The message must be durably stored — retrieve it via a pull consumer
        let stream_name = format!("telegram_commands_{}", prefix);
        let stream = js.get_stream(&stream_name).await.unwrap();
        let consumer = stream
            .get_or_create_consumer(
                "verify",
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some("verify".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let mut messages = consumer.messages().await.unwrap();
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — command was not persisted in JetStream stream")
        .unwrap()
        .unwrap();

        // Payload must round-trip correctly
        let received: TestMsg = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(received.value, "outbound");
        assert_eq!(received.count, 1);

        // CommandMetadata headers must be present
        let headers = msg.headers.as_ref().expect("headers must be present");
        let cmd_meta = read_cmd_metadata(headers).expect("X-Command-Id header must be present");
        assert_eq!(
            cmd_meta.command_id, expected_cmd_id,
            "command_id must match the one used to publish"
        );
        assert_eq!(
            cmd_meta.session_id.as_deref(),
            Some("session:42"),
            "session_id header must round-trip"
        );

        msg.ack().await.unwrap();
    }

    /// Outbound at-least-once: a command published via JetStream before any bot
    /// consumer exists must still be retrievable when the consumer connects later.
    ///
    /// This is the core at-least-once guarantee for the agent→bot direction.
    #[tokio::test]
    async fn test_outbound_command_survives_before_consumer_exists() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let js = async_nats::jetstream::new(client.clone());
        let prefix = format!("cmdsurvive-{}", uuid::Uuid::new_v4().simple());

        crate::nats::setup_agent_stream(&js, &prefix).await.unwrap();

        // Publish command — NO consumer exists yet (bot not started)
        let publisher = MessagePublisher::with_jetstream(client.clone(), js.clone(), &prefix);
        let subject = crate::subjects::agent::message_send(&prefix);
        publisher
            .publish_command(
                &subject,
                &TestMsg {
                    value: "survive".into(),
                    count: 99,
                },
                telegram_types::commands::CommandMetadata::new(),
            )
            .await
            .unwrap();

        // Simulate bot starting up later
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Bot creates its durable outbound consumer
        let consumer = crate::nats::create_outbound_consumer(&js, &prefix)
            .await
            .unwrap();
        let mut messages = consumer.messages().await.unwrap();

        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            futures::StreamExt::next(&mut messages),
        )
        .await
        .expect("timed out — outbound command was not retained in JetStream stream")
        .unwrap()
        .unwrap();

        let received: TestMsg = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(
            received.value, "survive",
            "command published before consumer must be delivered to the bot"
        );
        msg.ack().await.unwrap();
    }

    /// `publish_command` without a JetStream context (plain NATS fallback) must
    /// still succeed without panicking; it emits a warning but does not error.
    #[tokio::test]
    async fn test_publish_command_plain_nats_fallback_does_not_error() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.fallback.{}", uuid::Uuid::new_v4().simple());

        // No JetStream context — uses plain publish fallback
        let publisher = MessagePublisher::new(client.clone(), "test");
        let mut raw_sub = client.subscribe(subject.clone()).await.unwrap();

        publisher
            .publish_command(
                &subject,
                &TestMsg {
                    value: "fallback".into(),
                    count: 0,
                },
                telegram_types::commands::CommandMetadata::new(),
            )
            .await
            .expect("plain NATS fallback must not return an error");

        // Message is received via plain NATS (no JetStream retention)
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            futures::StreamExt::next(&mut raw_sub),
        )
        .await
        .expect("timed out — fallback publish not received")
        .unwrap();

        let received: TestMsg = serde_json::from_slice(&msg.payload).unwrap();
        assert_eq!(received.value, "fallback");
    }
}
