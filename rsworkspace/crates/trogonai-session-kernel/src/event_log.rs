use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use async_nats::jetstream::stream;
use buffa::Message as _;
use bytes::Bytes;
use futures::StreamExt as _;
use trogon_nats::jetstream::message::JsMessageRef;
use trogon_nats::jetstream::{
    JetStreamConsumer, JetStreamCreateConsumer, JetStreamGetStream, JetStreamPublisher,
};
use trogonai_session_contracts::{SessionEvent, SessionId, ValidatedSessionEvent};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::nats::{session_events_stream, session_events_subject, session_events_wildcard};

pub trait EventLogBackend: Send + Sync + Clone + 'static {
    fn append(
        &self,
        event: SessionEvent,
    ) -> impl std::future::Future<Output = Result<SessionEvent, SessionKernelError>> + Send;

    fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> impl std::future::Future<Output = Result<Vec<SessionEvent>, SessionKernelError>> + Send;

    fn last_seq(
        &self,
        session_id: &SessionId,
    ) -> impl std::future::Future<Output = Result<u64, SessionKernelError>> + Send;

    fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> impl std::future::Future<Output = Result<Option<SessionEvent>, SessionKernelError>> + Send;
}

/// Append-only JetStream event log for session events.
#[derive(Clone)]
pub struct EventLog<P, G> {
    publisher: P,
    stream_getter: G,
    stream_name: String,
    config: SessionKernelConfig,
}

impl<P, G> EventLog<P, G> {
    pub fn new(publisher: P, stream_getter: G, config: SessionKernelConfig) -> Self {
        Self {
            publisher,
            stream_getter,
            stream_name: session_events_stream(&config.nats_prefix),
            config,
        }
    }

    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }
}

impl<P, G> EventLog<P, G>
where
    P: JetStreamPublisher + Clone + Send + Sync + 'static,
    G: JetStreamGetStream + Clone + Send + Sync + 'static,
    G::Stream: JetStreamCreateConsumer + Send + Sync + 'static,
    <G::Stream as JetStreamCreateConsumer>::Consumer: JetStreamConsumer + Send + Sync + 'static,
    <<G::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message:
        JsMessageRef + Send + Sync,
{
    pub async fn provision_stream(&self) -> Result<(), SessionKernelError> {
        let _ = stream::Config {
            name: self.stream_name.clone(),
            subjects: vec![session_events_wildcard().to_string()],
            storage: stream::StorageType::File,
            retention: stream::RetentionPolicy::Limits,
            max_message_size: self.config.max_event_payload_bytes as i32,
            ..Default::default()
        };
        Ok(())
    }

    pub async fn append(&self, event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        let validated = ValidatedSessionEvent::try_from_event(event)?;
        let payload = validated.event.encode_to_vec();
        if payload.len() > self.config.max_event_payload_bytes {
            return Err(SessionKernelError::EventPayloadTooLarge {
                actual: payload.len(),
                limit: self.config.max_event_payload_bytes,
            });
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            NATS_MESSAGE_ID,
            async_nats::HeaderValue::from(validated.idempotency_key.as_str()),
        );

        let subject = session_events_subject(&validated.session_id);
        self.publisher
            .publish_with_headers(subject, headers, Bytes::from(payload))
            .await
            .map_err(|err| SessionKernelError::EventPublish(err.to_string()))?
            .await
            .map_err(|err| SessionKernelError::EventPublish(err.to_string()))?;

        Ok(validated.event)
    }

    pub async fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEvent>, SessionKernelError> {
        let filter = session_events_subject(session_id);
        let stream = self
            .stream_getter
            .get_stream(&self.stream_name)
            .await
            .map_err(|err| SessionKernelError::EventRead(err.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                filter_subject: filter,
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::None,
                ..Default::default()
            })
            .await
            .map_err(|err| SessionKernelError::EventRead(err.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|err| SessionKernelError::EventRead(err.to_string()))?;

        let mut events = Vec::new();
        while let Some(result) = messages.next().await {
            let message = result.map_err(|err| SessionKernelError::EventRead(err.to_string()))?;
            let payload = message.message().payload.clone();
            let event = SessionEvent::decode_from_slice(&payload)
                .map_err(|err| SessionKernelError::Decode(err.to_string()))?;
            events.push(event);
        }

        events.sort_by_key(|event| event.seq);
        Ok(events)
    }

    pub async fn last_seq(&self, session_id: &SessionId) -> Result<u64, SessionKernelError> {
        let events = self.read_session_events(session_id).await?;
        Ok(events.iter().map(|event| event.seq).max().unwrap_or(0))
    }

    pub async fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<SessionEvent>, SessionKernelError> {
        let events = self.read_session_events(session_id).await?;
        Ok(events
            .into_iter()
            .find(|event| event.idempotency_key == idempotency_key))
    }
}

impl<P, G> EventLogBackend for EventLog<P, G>
where
    P: JetStreamPublisher + Clone + Send + Sync + 'static,
    G: JetStreamGetStream + Clone + Send + Sync + 'static,
    G::Stream: JetStreamCreateConsumer + Send + Sync + 'static,
    <G::Stream as JetStreamCreateConsumer>::Consumer: JetStreamConsumer + Send + Sync + 'static,
    <<G::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message:
        JsMessageRef + Send + Sync,
{
    async fn append(&self, event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        EventLog::append(self, event).await
    }

    async fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEvent>, SessionKernelError> {
        EventLog::read_session_events(self, session_id).await
    }

    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, SessionKernelError> {
        EventLog::last_seq(self, session_id).await
    }

    async fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<SessionEvent>, SessionKernelError> {
        EventLog::find_by_idempotency_key(self, session_id, idempotency_key).await
    }
}

/// In-memory event log used by unit tests and crash/retry simulations.
#[derive(Clone, Default)]
pub struct InMemoryEventLog {
    events: Arc<Mutex<HashMap<String, Vec<SessionEvent>>>>,
}

impl InMemoryEventLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn append(&self, event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        let validated = ValidatedSessionEvent::try_from_event(event)?;
        let mut events = self.events.lock().unwrap();
        let bucket = events
            .entry(validated.session_id.as_str().to_string())
            .or_default();
        if let Some(existing) = bucket
            .iter()
            .find(|event| event.idempotency_key == validated.idempotency_key.as_str())
        {
            return Ok(existing.clone());
        }
        bucket.push(validated.event.clone());
        bucket.sort_by_key(|event| event.seq);
        Ok(validated.event)
    }

    pub async fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEvent>, SessionKernelError> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .get(session_id.as_str())
            .cloned()
            .unwrap_or_default())
    }

    pub async fn last_seq(&self, session_id: &SessionId) -> Result<u64, SessionKernelError> {
        let events = self.read_session_events(session_id).await?;
        Ok(events.iter().map(|event| event.seq).max().unwrap_or(0))
    }

    pub async fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<SessionEvent>, SessionKernelError> {
        let events = self.read_session_events(session_id).await?;
        Ok(events
            .into_iter()
            .find(|event| event.idempotency_key == idempotency_key))
    }
}

impl EventLogBackend for InMemoryEventLog {
    async fn append(&self, event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        InMemoryEventLog::append(self, event).await
    }

    async fn read_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEvent>, SessionKernelError> {
        InMemoryEventLog::read_session_events(self, session_id).await
    }

    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, SessionKernelError> {
        InMemoryEventLog::last_seq(self, session_id).await
    }

    async fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<SessionEvent>, SessionKernelError> {
        InMemoryEventLog::find_by_idempotency_key(self, session_id, idempotency_key).await
    }
}
