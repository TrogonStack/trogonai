//! Message publishing and subscription helpers

use async_nats::Client;
use futures::StreamExt;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, error, trace};

use crate::error::{Error, Result};

/// Trait for publishing messages to a transport layer.
/// Implemented by `MessagePublisher` (real NATS) and `MockPublisher` (in-memory, tests).
#[allow(async_fn_in_trait)]
pub trait Publish {
    fn prefix(&self) -> &str;

    async fn publish<T: serde::Serialize>(
        &self,
        subject: impl AsRef<str>,
        message: &T,
    ) -> crate::error::Result<()>;
}

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
}

impl Publish for MessagePublisher {
    fn prefix(&self) -> &str {
        self.prefix()
    }

    async fn publish<T: serde::Serialize>(
        &self,
        subject: impl AsRef<str>,
        message: &T,
    ) -> crate::error::Result<()> {
        MessagePublisher::publish(self, subject, message).await
    }
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

    /// Subscribe with a queue group for load-balanced consumption
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
        let subject = format!(
            "test.dc.messaging.roundtrip.{}",
            uuid::Uuid::new_v4().simple()
        );

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
        let _ = subscriber.client();
    }

    #[tokio::test]
    async fn test_deserialize_error_on_invalid_json() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!(
            "test.dc.messaging.invalid.{}",
            uuid::Uuid::new_v4().simple()
        );

        let subscriber = MessageSubscriber::new(client.clone(), "test");
        let mut stream = subscriber.subscribe::<TestMsg>(&subject).await.unwrap();

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
        let subject = format!("test.dc.messaging.raw.{}", uuid::Uuid::new_v4().simple());

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
    async fn test_queue_subscribe_roundtrip() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let subject = format!("test.dc.messaging.queue.{}", uuid::Uuid::new_v4().simple());

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
}
