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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StreamPosition;

    #[test]
    fn display_and_source_preserve_envelope_decode_error() {
        let source = serde_json::from_slice::<serde_json::Value>(b"not-json").unwrap_err();
        let source_message = source.to_string();
        let error = SnapshotEnvelopeDecodeError::envelope_source(source);

        assert_eq!(
            error.to_string(),
            format!("failed to decode snapshot envelope: {source_message}")
        );
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(source_message)
        );
    }

    #[test]
    fn display_and_source_preserve_position_decode_error() {
        let source = StreamPosition::try_new(0).unwrap_err();
        let source_message = source.to_string();
        let error = SnapshotEnvelopeDecodeError::position_source(source);

        assert_eq!(
            error.to_string(),
            format!("failed to decode snapshot position: {source_message}")
        );
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(source_message)
        );
    }
}
