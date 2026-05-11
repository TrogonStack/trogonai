pub trait EventCodec<T> {
    type Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, event_type: &str, stream_id: &str, payload: &[u8]) -> Result<T, Self::Error>;
}
