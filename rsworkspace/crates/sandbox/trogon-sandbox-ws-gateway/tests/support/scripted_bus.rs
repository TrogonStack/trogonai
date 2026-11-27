use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use trogon_sandbox_client::{Bus, BusError, BusMessage};

/// What the scripted bus does with one request.
#[derive(Clone)]
pub enum Reply {
    /// No answer arrives before the timeout.
    Silent,
    /// The bus itself fails.
    Fail,
    /// The broker answers with these bytes.
    Bytes(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
#[error("the scripted bus failed")]
struct ScriptedBusError;

/// A bus whose answers are scripted: queued replies first, then `fallback`. Subscriptions replay `messages`.
pub struct ScriptedBus {
    replies: Mutex<VecDeque<Reply>>,
    fallback: Reply,
    messages: Vec<BusMessage>,
    subscribe_fails: bool,
}

impl ScriptedBus {
    /// A bus nobody answers on.
    pub fn silent() -> Self {
        Self::answering(Reply::Silent)
    }

    pub fn answering(fallback: Reply) -> Self {
        Self {
            replies: Mutex::new(VecDeque::new()),
            fallback,
            messages: Vec::new(),
            subscribe_fails: false,
        }
    }

    pub fn answering_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Ok(Self::answering(Reply::Bytes(serde_json::to_vec(value)?)))
    }

    /// Replies queued ahead of the fallback.
    pub fn then(self, reply: Reply) -> Self {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(reply);
        self
    }

    /// Messages every subscription delivers, then ends.
    pub fn publishing(mut self, messages: Vec<BusMessage>) -> Self {
        self.messages = messages;
        self
    }

    pub fn failing_subscriptions(mut self) -> Self {
        self.subscribe_fails = true;
        self
    }
}

#[async_trait]
impl Bus for ScriptedBus {
    async fn request(
        &self,
        _subject: &str,
        _payload: Bytes,
        _timeout: Duration,
        _headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        let next = self.replies.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
        match next.unwrap_or_else(|| self.fallback.clone()) {
            Reply::Silent => Ok(None),
            Reply::Fail => Err(BusError::Custom(Box::new(ScriptedBusError))),
            Reply::Bytes(bytes) => Ok(Some(Bytes::from(bytes))),
        }
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        if self.subscribe_fails {
            return Err(BusError::Custom(Box::new(ScriptedBusError)));
        }
        Ok(stream::iter(self.messages.clone()).boxed())
    }
}
