pub trait SnapshotPayloadEncode {
    type Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error>;
}
