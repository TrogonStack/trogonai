pub trait EventEncode {
    type Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error>;
}
