use crate::InvalidStreamPosition;

#[derive(Debug)]
pub enum SnapshotEnvelopeDecodeError {
    Envelope { source: serde_json::Error },
    Position { source: InvalidStreamPosition },
}

impl SnapshotEnvelopeDecodeError {
    pub(super) fn envelope_source(source: serde_json::Error) -> Self {
        Self::Envelope { source }
    }

    pub(super) fn position_source(source: InvalidStreamPosition) -> Self {
        Self::Position { source }
    }
}

impl std::fmt::Display for SnapshotEnvelopeDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Envelope { source } => write!(f, "failed to decode snapshot envelope: {source}"),
            Self::Position { source } => write!(f, "failed to decode snapshot position: {source}"),
        }
    }
}

impl std::error::Error for SnapshotEnvelopeDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Envelope { source } => Some(source),
            Self::Position { source } => Some(source),
        }
    }
}
