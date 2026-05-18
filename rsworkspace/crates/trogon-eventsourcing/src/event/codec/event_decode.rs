pub trait EventDecode: Sized {
    type Error;

    fn decode(event_type: &str, stream_id: &str, payload: &[u8]) -> Result<Self, Self::Error>;
}
