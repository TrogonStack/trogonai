//! In-memory mock publisher for unit testing without a real NATS connection.

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::error::Result;
use crate::messaging::Publish;

/// Captured publish: (full subject string, JSON value of the message)
pub type CapturedMessage = (String, serde_json::Value);

/// In-memory publisher that records all published messages.
/// Use in tests instead of a real `MessagePublisher`.
///
/// # Example
/// ```rust,ignore
/// let mock = MockPublisher::new("test-prefix");
/// processor.process_message(&event, &mock).await.unwrap();
/// let msgs = mock.published_messages();
/// assert_eq!(msgs[0].0, "test-prefix.discord.guild.bot.message.created");
/// ```
#[derive(Clone)]
pub struct MockPublisher {
    prefix: String,
    messages: Arc<Mutex<Vec<CapturedMessage>>>,
}

impl MockPublisher {
    /// Create a new mock publisher with the given prefix.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return a snapshot of all captured (subject, value) pairs in publish order.
    pub fn published_messages(&self) -> Vec<CapturedMessage> {
        self.messages.lock().unwrap().clone()
    }

    /// Return the number of messages published so far.
    pub fn message_count(&self) -> usize {
        self.messages.lock().unwrap().len()
    }

    /// Clear all captured messages.
    pub fn clear(&self) {
        self.messages.lock().unwrap().clear();
    }

    /// Return true if no messages have been published.
    pub fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }
}

impl Publish for MockPublisher {
    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn publish<T: Serialize>(
        &self,
        subject: impl AsRef<str>,
        message: &T,
    ) -> Result<()> {
        let subject = subject.as_ref().to_string();
        let value = serde_json::to_value(message)
            .map_err(crate::error::Error::Serialization)?;
        self.messages.lock().unwrap().push((subject, value));
        Ok(())
    }
}
