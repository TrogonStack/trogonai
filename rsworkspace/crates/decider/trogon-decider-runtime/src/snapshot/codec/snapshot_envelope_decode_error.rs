use crate::InvalidStreamPosition;

/// Error returned by [`EncodedSnapshot::from_bytes`](super::EncodedSnapshot::from_bytes)
/// when the on-the-wire envelope cannot be parsed back into an [`EncodedSnapshot`](super::EncodedSnapshot).
#[derive(Debug, thiserror::Error)]
pub enum SnapshotEnvelopeDecodeError {
    /// The envelope bytes are not valid JSON for the expected shape.
    #[error("failed to decode snapshot envelope: {source}")]
    Envelope {
        /// The underlying JSON deserialization failure.
        #[source]
        source: serde_json::Error,
    },
    /// The envelope's stored position is not a valid [`StreamPosition`](crate::StreamPosition).
    #[error("failed to decode snapshot position: {source}")]
    Position {
        /// The underlying position validation failure.
        #[source]
        source: InvalidStreamPosition,
    },
}

impl SnapshotEnvelopeDecodeError {
    pub(super) fn envelope_source(source: serde_json::Error) -> Self {
        Self::Envelope { source }
    }

    pub(super) fn position_source(source: InvalidStreamPosition) -> Self {
        Self::Position { source }
    }
}

#[cfg(test)]
mod tests;
