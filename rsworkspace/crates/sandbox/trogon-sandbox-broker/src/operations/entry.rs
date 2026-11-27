use bytes::Bytes;

/// A value read from an operation store together with its revision.
#[derive(Debug, Clone)]
pub struct Entry {
    pub value: Bytes,
    pub revision: u64,
}
