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
        let payload = serde_json::to_vec(message).map_err(|e| Error::Serialization(e))?;

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
        let payload = serde_json::to_vec(message).map_err(|e| Error::Serialization(e))?;

        trace!("Publishing to subject: {} with headers", subject);

        self.client
            .publish_with_headers(subject.to_string(), headers, payload.into())
            .await
            .map_err(|e| Error::Publish(format!("Failed to publish to {}: {}", subject, e)))?;

        debug!("Published message with headers to {}", subject);
        Ok(())
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

    #[test]
    fn test_message_publisher_creation() {
        // This test just verifies the types compile correctly
        // Actual connection tests would require a running NATS server
    }
}
