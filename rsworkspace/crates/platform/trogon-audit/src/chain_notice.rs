/// Something a verifier saw that is not tampering but that a reader should know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainNotice {
    /// The previous record was stored again with identical bytes. A writer retries
    /// a record whose acknowledgement it lost with the same bytes, and past the
    /// stream's duplicate window the stream stores the retry as a second message.
    /// The copies carry the same content, so the chain is intact.
    IdenticalDuplicate {
        sequence: u64,
        stream_sequence: u64,
        previous_stream_sequence: u64,
    },
}
