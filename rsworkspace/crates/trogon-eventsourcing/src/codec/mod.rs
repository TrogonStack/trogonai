use crate::EventId;

pub trait EventCodec<T> {
    type Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, event_type: &str, stream_id: &str, payload: &[u8]) -> Result<T, Self::Error>;
}

pub trait CanonicalEventCodec: Sized {
    type Codec: EventCodec<Self>;

    fn canonical_codec() -> Self::Codec;
}

pub trait EventIdentity {
    fn event_id(&self) -> Option<EventId> {
        None
    }
}

pub trait EventType {
    type Error;

    fn event_type(&self) -> Result<&'static str, Self::Error>;
}
