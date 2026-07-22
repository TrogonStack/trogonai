/// Error returned by [`EncodedSnapshot::into_bytes`](super::EncodedSnapshot::into_bytes)
/// when an [`EncodedSnapshot`](super::EncodedSnapshot) cannot be serialized to its
/// on-the-wire envelope bytes.
#[derive(Debug, thiserror::Error)]
#[error("failed to encode snapshot envelope: {source}")]
pub struct SnapshotEnvelopeEncodeError {
    #[source]
    source: serde_json::Error,
}

impl SnapshotEnvelopeEncodeError {
    pub(super) fn new(source: serde_json::Error) -> Self {
        Self { source }
    }
}

#[cfg(test)]
mod tests;
