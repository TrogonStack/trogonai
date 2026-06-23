use crate::InvalidStreamPosition;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotEnvelopeDecodeError {
    #[error("failed to decode snapshot envelope: {source}")]
    Envelope {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to decode snapshot position: {source}")]
    Position {
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
