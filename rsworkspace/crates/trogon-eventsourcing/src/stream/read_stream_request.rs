#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamRequest<'a, StreamId: ?Sized> {
    pub stream_id: &'a StreamId,
    pub from_sequence: u64,
}

impl<'a, StreamId: ?Sized> ReadStreamRequest<'a, StreamId> {
    pub const fn new(stream_id: &'a StreamId, from_sequence: u64) -> Self {
        Self {
            stream_id,
            from_sequence,
        }
    }
}
