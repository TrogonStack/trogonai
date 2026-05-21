pub trait SnapshotPayloadEncode {
    type Error: std::error::Error + Send + Sync + 'static;

    fn encode(&self) -> Result<Vec<u8>, Self::Error>;
}
