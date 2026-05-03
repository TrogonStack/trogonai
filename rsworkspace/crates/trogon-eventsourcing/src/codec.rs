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

pub trait EventIdentity {
    fn event_id(&self) -> Option<EventId> {
        None
    }
}

pub trait EventType {
    fn event_type(&self) -> &'static str;
}
