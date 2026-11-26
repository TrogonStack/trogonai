//! Shared helpers for the client tests: a scriptable in-memory bus and small constructors.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use serde::Serialize;
use trogon_sandbox_client::{Bus, BusError, BusMessage, SandboxClient};
use trogon_sandbox_contract::{LeaseId, TenantId};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One request the client sent through the bus.
#[derive(Debug, Clone)]
pub struct Sent {
    pub subject: String,
    pub payload: Bytes,
    pub timeout: Duration,
    pub headers: Option<async_nats::HeaderMap>,
}

impl Sent {
    /// The request body decoded as JSON.
    pub fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }

    /// The `Authorization` header value, when one was sent.
    pub fn authorization(&self) -> Option<String> {
        self.headers
            .as_ref()?
            .get("Authorization")
            .map(|value| value.to_string())
    }
}

/// A shared, inspectable list of the requests a [`MockBus`] received.
#[derive(Debug, Clone, Default)]
pub struct SentLog(Arc<Mutex<Vec<Sent>>>);

impl SentLog {
    pub fn all(&self) -> Vec<Sent> {
        self.0.lock().map(|sent| sent.clone()).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.all().len()
    }

    pub fn is_empty(&self) -> bool {
        self.all().is_empty()
    }

    /// The only request sent, or an error when there were none or several.
    pub fn only(&self) -> Result<Sent, String> {
        let mut all = self.all();
        match all.len() {
            1 => Ok(all.remove(0)),
            count => Err(format!("expected exactly one request, saw {count}")),
        }
    }

    fn push(&self, sent: Sent) {
        if let Ok(mut all) = self.0.lock() {
            all.push(sent);
        }
    }
}

/// What a [`MockBus`] does with one request.
#[derive(Debug, Clone)]
pub enum Reply {
    Bytes(Bytes),
    Timeout,
    Fail,
}

impl Reply {
    pub fn json(value: &impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::Bytes(Bytes::from(serde_json::to_vec(value)?)))
    }
}

/// A bus that answers requests from a script. The last scripted reply repeats once the script runs out.
pub struct MockBus {
    script: Mutex<VecDeque<Reply>>,
    last: Mutex<Reply>,
    subscribe_items: Vec<BusMessage>,
    log: SentLog,
}

impl MockBus {
    pub fn scripted(replies: Vec<Reply>) -> Self {
        let last = replies.last().cloned().unwrap_or(Reply::Timeout);
        Self {
            script: Mutex::new(replies.into()),
            last: Mutex::new(last),
            subscribe_items: Vec::new(),
            log: SentLog::default(),
        }
    }

    pub fn replying(bytes: Bytes) -> Self {
        Self::scripted(vec![Reply::Bytes(bytes)])
    }

    pub fn replying_json(value: &impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::scripted(vec![Reply::json(value)?]))
    }

    pub fn timing_out() -> Self {
        Self::scripted(vec![Reply::Timeout])
    }

    pub fn failing() -> Self {
        Self::scripted(vec![Reply::Fail])
    }

    pub fn with_subscribe_items(items: Vec<BusMessage>) -> Self {
        let mut bus = Self::timing_out();
        bus.subscribe_items = items;
        bus
    }

    pub fn log(&self) -> SentLog {
        self.log.clone()
    }

    pub fn into_client(self) -> SandboxClient {
        SandboxClient::from_bus(Box::new(self))
    }
}

#[async_trait]
impl Bus for MockBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        timeout: Duration,
        headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        self.log.push(Sent {
            subject: subject.to_string(),
            payload,
            timeout,
            headers,
        });
        let reply = {
            let mut script = self.script.lock().map_err(poisoned)?;
            match script.pop_front() {
                Some(reply) => reply,
                None => self.last.lock().map_err(poisoned)?.clone(),
            }
        };
        match reply {
            Reply::Bytes(bytes) => Ok(Some(bytes)),
            Reply::Timeout => Ok(None),
            Reply::Fail => Err(BusError::Custom("bus down".into())),
        }
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        Ok(Box::pin(futures::stream::iter(self.subscribe_items.clone())))
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> BusError {
    BusError::Custom("mock bus state poisoned".into())
}

/// A client whose bus answers every request with `response` as JSON, plus the log of what it was sent.
pub fn client_replying(response: &impl Serialize) -> Result<(SandboxClient, SentLog), serde_json::Error> {
    let bus = MockBus::replying_json(response)?;
    let log = bus.log();
    Ok((bus.into_client(), log))
}

pub fn tenant() -> Result<TenantId, trogon_sandbox_contract::TenantIdError> {
    TenantId::new("tenant-1")
}

pub fn lease_id() -> LeaseId {
    LeaseId::new("lease-1")
}
