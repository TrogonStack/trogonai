/// Encoded snapshot payload bytes handed to [`SnapshotPayloadDecode::decode`].
///
/// `#[non_exhaustive]` so new fields can be added later without breaking
/// callers that construct or match on this type.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SnapshotPayloadData<'a> {
    /// The encoded payload bytes to decode.
    pub payload: &'a [u8],
}

impl<'a> SnapshotPayloadData<'a> {
    /// Wraps encoded payload bytes for a decode call.
    pub const fn new(payload: &'a [u8]) -> Self {
        Self { payload }
    }
}

/// Decodes a decider state from its encoded snapshot payload bytes.
pub trait SnapshotPayloadDecode: Sized {
    /// Error returned when the payload bytes cannot be decoded.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Decodes a decider state from the given payload bytes.
    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error>;
}
