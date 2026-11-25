use crate::constants::SEAL_TRAILER_BYTES;

/// Why stored bytes are not a sealed record: the trailer is missing or does not
/// hash the bytes before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    #[error("sealed record must be at least {SEAL_TRAILER_BYTES} bytes, got {0}")]
    TooShort(usize),
    #[error("sealed record does not end in a record hash trailer")]
    MissingTrailer,
    #[error("record hash does not match the record's bytes")]
    HashMismatch,
}

#[cfg(test)]
mod tests;
