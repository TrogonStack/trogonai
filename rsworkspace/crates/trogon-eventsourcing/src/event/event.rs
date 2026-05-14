use trogon_std::UuidV7Generator;

use super::{EventHeaders, StreamEvent};
use crate::{EncodeEventError, EventCodec, EventEncodeError, EventId, EventIdentity, EventType, StreamPosition};

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub r#type: String,
    pub content: Vec<u8>,
    pub headers: EventHeaders,
}

impl Event {
    pub fn from_domain_event<E, C>(codec: &C, event: &E) -> Result<Self, EventEncodeError<E, C>>
    where
        E: EventType + EventIdentity,
        C: EventCodec<E>,
    {
        let id = event.event_id().unwrap_or_else(|| EventId::now_v7(&UuidV7Generator));
        Ok(Self {
            id,
            r#type: event.event_type().map_err(EncodeEventError::EventType)?.to_string(),
            content: codec.encode(event).map_err(EncodeEventError::EventCodec)?,
            headers: EventHeaders::empty(),
        })
    }

    pub fn with_headers(mut self, headers: EventHeaders) -> Self {
        self.headers = headers;
        self
    }

    pub fn record(
        self,
        stream_id: impl Into<String>,
        stream_position: StreamPosition,
        recorded_at: chrono::DateTime<chrono::Utc>,
    ) -> StreamEvent {
        StreamEvent {
            stream_id: stream_id.into(),
            event: self,
            stream_position,
            recorded_at,
        }
    }

    pub fn decode_with<E, C>(&self, stream_id: &str, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        codec.decode(&self.r#type, stream_id, &self.content)
    }
}
