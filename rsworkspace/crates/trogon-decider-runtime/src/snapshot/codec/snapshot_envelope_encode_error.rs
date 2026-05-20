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
