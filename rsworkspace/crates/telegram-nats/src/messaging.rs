//! Message publishing and subscription helpers

use async_nats::Client;
use futures::StreamExt;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, error, trace};

use crate::error::{Error, Result};

/// Message publisher for sending events and commands
#[derive(Clone)]
pub struct MessagePublisher {
    client: Client,
    prefix: String,
}

impl MessagePublisher {
    /// Create a new message publisher
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
    pub async fn publish_command<T: Serialize>(
        &self,
        subject: impl AsRef<str>,
        command: &T,
        metadata: telegram_types::commands::CommandMetadata,
    ) -> Result<()> {
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
        self.publish_with_headers(subject, command, headers).await
    }
}

/// Read `CommandMetadata` fields from NATS message headers.
///
/// Returns `None` if `X-Command-Id` is absent or unparseable.
pub fn read_cmd_metadata(
    headers: &async_nats::HeaderMap,
) -> Option<telegram_types::commands::CommandMetadata> {
    use std::str::FromStr;
    let command_id = uuid::Uuid::from_str(
        headers.get("X-Command-Id")?.as_str(),
    )
    .ok()?;

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
        causation_id: headers.get("X-Causation-Id").map(|v| v.as_str().to_string()),
        correlation_id: headers.get("X-Correlation-Id").map(|v| v.as_str().to_string()),
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
}
