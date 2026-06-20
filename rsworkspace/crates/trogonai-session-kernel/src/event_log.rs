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
    JetStreamConsumer, JetStreamContext, JetStreamCreateConsumer, JetStreamGetStream, JetStreamPublisher,
};
use trogonai_session_contracts::{SessionEvent, SessionId, ValidatedSessionEvent};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::nats::{session_events_stream, session_events_subject, session_events_wildcard};
use crate::policies::NatsOperationalPolicy;

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

    /// Provision the append-only JetStream event-log stream with the NATS operational
    /// policy (§3 JetStream event log; § NATS Operational Policy): durable File
    /// storage, `Limits` retention, max message size, max bytes and replicas. Uses
    /// `get_or_create_stream`, so it is idempotent and safe to call on every startup.
    pub async fn provision_stream<Ctx>(
        &self,
        context: &Ctx,
        policy: &NatsOperationalPolicy,
    ) -> Result<(), SessionKernelError>
    where
        Ctx: JetStreamContext,
    {
        let config = stream::Config {
            name: self.stream_name.clone(),
            subjects: vec![session_events_wildcard().to_string()],
            storage: stream::StorageType::File,
            retention: stream::RetentionPolicy::Limits,
            max_message_size: policy.event_stream_max_message_size,
            max_bytes: policy.event_stream_max_bytes,
            num_replicas: policy.event_stream_replicas.max(1) as usize,
            ..Default::default()
        };
        context
            .get_or_create_stream(config)
            .await
            .map_err(|err| SessionKernelError::Provision(err.to_string()))?;
        Ok(())
    }
}

impl<P, G> EventLog<P, G>
where
    P: JetStreamPublisher + Clone + Send + Sync + 'static,
    G: JetStreamGetStream + Clone + Send + Sync + 'static,
    G::Stream: JetStreamCreateConsumer + Send + Sync + 'static,
    <G::Stream as JetStreamCreateConsumer>::Consumer: JetStreamConsumer + Send + Sync + 'static,
    <<G::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message: JsMessageRef + Send + Sync,
{
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

    pub async fn read_session_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>, SessionKernelError> {
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

        // Bound the read to the messages currently in the stream (§ append "load last
        // session seq" / reconstruccion "leer eventos con seq > last_applied_seq"). A
        // pull consumer's `messages()` stream is CONTINUOUS — `next()` blocks waiting for
        // future events once the subject is drained — so without this bound the read
        // would hang forever on an exhausted (or empty) session.
        let pending = consumer.num_pending().await;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|err| SessionKernelError::EventRead(err.to_string()))?;

        let mut events = Vec::new();
        let mut read = 0u64;
        while read < pending {
            let Some(result) = messages.next().await else {
                break;
            };
            let message = result.map_err(|err| SessionKernelError::EventRead(err.to_string()))?;
            let payload = message.message().payload.clone();
            let event =
                SessionEvent::decode_from_slice(&payload).map_err(|err| SessionKernelError::Decode(err.to_string()))?;
            events.push(event);
            read += 1;
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
    <<G::Stream as JetStreamCreateConsumer>::Consumer as JetStreamConsumer>::Message: JsMessageRef + Send + Sync,
{
    async fn append(&self, event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        EventLog::append(self, event).await
    }

    async fn read_session_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>, SessionKernelError> {
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
        let bucket = events.entry(validated.session_id.as_str().to_string()).or_default();
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

    pub async fn read_session_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>, SessionKernelError> {
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

    async fn read_session_events(&self, session_id: &SessionId) -> Result<Vec<SessionEvent>, SessionKernelError> {
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

#[cfg(test)]
mod provision_tests {
    use super::*;
    use crate::policies::SessionKernelOperationalPolicy;
    use trogon_nats::jetstream::MockJetStreamContext;

    #[tokio::test]
    async fn provision_stream_creates_event_log_stream_with_nats_policy() {
        // provision_stream must actually create the stream (not discard the config)
        // with the NATS operational policy (§3; § NATS Operational Policy).
        let event_log = EventLog::<(), ()>::new((), (), SessionKernelConfig::default());
        let context = MockJetStreamContext::new();
        let policy = SessionKernelOperationalPolicy::default();

        event_log
            .provision_stream(&context, &policy.nats)
            .await
            .expect("provision the event-log stream");

        let created = context.created_streams();
        assert_eq!(created.len(), 1, "the event-log stream must be created");
        let cfg = &created[0];
        assert_eq!(cfg.max_bytes, policy.nats.event_stream_max_bytes);
        assert_eq!(cfg.num_replicas, policy.nats.event_stream_replicas.max(1) as usize);
        assert_eq!(cfg.retention, stream::RetentionPolicy::Limits);
        assert!(
            cfg.subjects.iter().any(|subject| subject.contains("events")),
            "subjects must cover the session events wildcard, got {:?}",
            cfg.subjects
        );
    }
}
