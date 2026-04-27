use serde::{Serialize, de::DeserializeOwned};

use crate::EventId;

pub trait EventCodec<T> {
    type Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, event_type: &str, stream_id: &str, payload: &[u8]) -> Result<T, Self::Error>;
}

pub trait EventEnvelopeCodec<T>: EventCodec<T> {
    fn event_type(&self, value: &T) -> Result<&'static str, Self::Error>;

    fn event_id(&self, _value: &T) -> Option<EventId> {
        None
    }
}

pub trait CanonicalEventCodec: Sized {
    type Codec: EventEnvelopeCodec<Self>;

    fn canonical_codec() -> Self::Codec;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonEventCodec;

impl<T> EventCodec<T> for JsonEventCodec
where
    T: Serialize + DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(payload)
    }
}

impl<T> EventEnvelopeCodec<T> for JsonEventCodec
where
    T: EventType + EventIdentity + Serialize + DeserializeOwned,
{
    fn event_type(&self, value: &T) -> Result<&'static str, Self::Error> {
        Ok(value.event_type())
    }

    fn event_id(&self, value: &T) -> Option<EventId> {
        value.event_id()
    }
}

pub trait EventIdentity {
    fn event_id(&self) -> Option<EventId> {
        None
    }
}

pub trait EventType {
    fn event_type(&self) -> &'static str;
}
