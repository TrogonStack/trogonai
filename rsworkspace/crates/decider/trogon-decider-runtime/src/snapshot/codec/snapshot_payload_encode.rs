/// Encodes a decider state into bytes suitable for a snapshot payload.
pub trait SnapshotPayloadEncode {
    /// Error returned when the state cannot be encoded.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Encodes this decider state into payload bytes.
    fn encode(&self) -> Result<Vec<u8>, Self::Error>;
}
