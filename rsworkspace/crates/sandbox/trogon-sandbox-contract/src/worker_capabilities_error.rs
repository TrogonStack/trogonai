/// A worker capability bitset carries bits outside the known flags.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkerCapabilitiesError {
    #[error("unknown worker capability bits: {0}")]
    UnknownBits(u32),
}
