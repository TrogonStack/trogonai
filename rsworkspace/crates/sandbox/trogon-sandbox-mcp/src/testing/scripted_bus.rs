use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Serialize;
use tokio::sync::Notify;
use trogon_sandbox_client::{Bus, BusError, BusMessage, SandboxClient};

use super::{Sent, SentLog};
use crate::lock::lock;

/// What a scripted bus does with one request.
#[derive(Debug, Clone)]
pub(crate) enum Reply {
    Bytes(Bytes),
    Timeout,
}

impl Reply {
    pub(crate) fn json(value: &impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::Bytes(Bytes::from(serde_json::to_vec(value)?)))
    }
}

/// A bus that answers requests from a script. The last scripted reply repeats once the script runs out.
pub(crate) struct ScriptedBus {
    script: Mutex<VecDeque<Reply>>,
    last: Reply,
    subscribe_items: Vec<BusMessage>,
    gate: Option<StreamGate>,
    log: SentLog,
}

impl ScriptedBus {
    pub(crate) fn scripted(replies: Vec<Reply>) -> Self {
        Self {
            last: replies.last().cloned().unwrap_or(Reply::Timeout),
            script: Mutex::new(replies.into()),
            subscribe_items: Vec::new(),
            gate: None,
            log: SentLog::default(),
        }
    }

    pub(crate) fn replying(value: &impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::scripted(vec![Reply::json(value)?]))
    }

    pub(crate) fn silent() -> Self {
        Self::scripted(vec![Reply::Timeout])
    }

    pub(crate) fn with_subscribe_items(mut self, items: Vec<BusMessage>) -> Self {
        self.subscribe_items = items;
        self
    }

    /// Holds the subscribe stream back until the returned gate opens, and reports when the stream is dropped.
    pub(crate) fn with_gated_subscribe(mut self, items: Vec<BusMessage>) -> (Self, StreamGate) {
        let gate = StreamGate::default();
        self.subscribe_items = items;
        self.gate = Some(gate.clone());
        (self, gate)
    }

    pub(crate) fn log(&self) -> SentLog {
        self.log.clone()
    }

    pub(crate) fn into_client(self) -> SandboxClient {
        SandboxClient::from_bus(Box::new(self))
    }
}

#[async_trait]
impl Bus for ScriptedBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        _timeout: Duration,
        _headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        self.log.push(Sent {
            subject: subject.to_string(),
            payload,
        });
        let reply = lock(&self.script).pop_front().unwrap_or_else(|| self.last.clone());
        Ok(match reply {
            Reply::Bytes(bytes) => Some(bytes),
            Reply::Timeout => None,
        })
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        let items = futures::stream::iter(self.subscribe_items.clone());
        let Some(gate) = self.gate.clone() else {
            return Ok(Box::pin(items));
        };
        let dropped = DropSignal(gate.dropped.clone());
        let opened = gate.opened.clone();
        let held = futures::stream::once(async move { opened.notified().await }).flat_map(move |()| items.clone());
        Ok(Box::pin(held.map(move |message| {
            let _keep = &dropped;
            message
        })))
    }
}

/// Controls a gated subscribe stream from a test.
#[derive(Debug, Clone, Default)]
pub(crate) struct StreamGate {
    opened: Arc<Notify>,
    dropped: Arc<Notify>,
}

impl StreamGate {
    pub(crate) fn open(&self) {
        self.opened.notify_one();
    }

    /// Resolves once the server has dropped the stream.
    pub(crate) async fn dropped(&self) {
        self.dropped.notified().await;
    }
}

struct DropSignal(Arc<Notify>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}
