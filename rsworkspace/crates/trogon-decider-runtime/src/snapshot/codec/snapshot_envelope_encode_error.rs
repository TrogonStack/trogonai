#[derive(Debug)]
pub struct SnapshotEnvelopeEncodeError {
    source: serde_json::Error,
}

impl SnapshotEnvelopeEncodeError {
    pub(super) fn new(source: serde_json::Error) -> Self {
        Self { source }
    }
}

impl std::fmt::Display for SnapshotEnvelopeEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to encode snapshot envelope: {}", self.source)
    }
}

impl std::error::Error for SnapshotEnvelopeEncodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_source_preserve_envelope_encode_error() {
        let source = serde_json::from_slice::<serde_json::Value>(b"not-json").unwrap_err();
        let source_message = source.to_string();
        let error = SnapshotEnvelopeEncodeError::new(source);

        assert_eq!(
            error.to_string(),
            format!("failed to encode snapshot envelope: {source_message}")
        );
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some(source_message)
        );
    }
}
