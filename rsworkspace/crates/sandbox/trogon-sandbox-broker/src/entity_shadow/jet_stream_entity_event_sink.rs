use async_nats::jetstream;
use async_nats::jetstream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::EntityLifecycleEvent;
use trogon_sandbox_protocol::subjects::{ENTITY_EVENT_STREAM, ENTITY_EVENT_SUBJECT_PREFIX};

use super::{EntityEventSink, EntityEventSinkError, entity_event_subject};
use crate::constants::{ENTITY_EVENT_MAX_AGE, ENTITY_EVENTS_PER_SUBJECT};

pub struct JetStreamEntityEventSink {
    jetstream: jetstream::Context,
}

impl JetStreamEntityEventSink {
    pub async fn new(jetstream: jetstream::Context, num_replicas: usize) -> Result<Self, EntityEventSinkError> {
        jetstream
            .get_or_create_stream(stream::Config {
                name: ENTITY_EVENT_STREAM.to_string(),
                subjects: vec![format!("{ENTITY_EVENT_SUBJECT_PREFIX}.>")],
                retention: stream::RetentionPolicy::Limits,
                max_messages_per_subject: ENTITY_EVENTS_PER_SUBJECT,
                max_age: ENTITY_EVENT_MAX_AGE,
                num_replicas,
                storage: stream::StorageType::File,
                ..Default::default()
            })
            .await
            .map_err(EntityEventSinkError::backend)?;
        Ok(Self { jetstream })
    }
}

#[async_trait]
impl EntityEventSink for JetStreamEntityEventSink {
    async fn publish(&self, event: &EntityLifecycleEvent) -> Result<(), EntityEventSinkError> {
        let subject = entity_event_subject(event.tenant(), event.entity_id());
        let payload = serde_json::to_vec(event)?;
        self.jetstream
            .publish(subject, Bytes::from(payload))
            .await
            .map_err(EntityEventSinkError::backend)?
            .await
            .map_err(EntityEventSinkError::backend)?;
        Ok(())
    }
}
