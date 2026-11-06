/// Why a checkpoint was not signed. None of these touches the chain: the writer
/// goes on storing records, and the next checkpoint covers the head it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointSignError {
    #[error("checkpoint signer is unavailable")]
    Unavailable,
    #[error("checkpoint signer refused to sign")]
    Refused,
    /// The signer returned a signature its own key id does not verify. Publishing
    /// it would store a checkpoint no verifier accepts.
    #[error("checkpoint signature does not verify under the key id it came with")]
    InvalidSignature,
}

impl CheckpointSignError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Refused => "refused",
            Self::InvalidSignature => "invalid_signature",
        }
    }
}

#[cfg(test)]
mod tests;
