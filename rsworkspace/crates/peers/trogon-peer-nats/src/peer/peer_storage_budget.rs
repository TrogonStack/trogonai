#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerStorageBudget(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerStorageBudgetError {
    #[error("peer event storage budget must be positive")]
    Zero,
    #[error("peer event storage budget exceeds the supported byte limit")]
    OutOfRange,
}

impl PeerStorageBudget {
    pub fn new(bytes: u64) -> Result<Self, PeerStorageBudgetError> {
        if bytes == 0 {
            return Err(PeerStorageBudgetError::Zero);
        }
        if bytes > i64::MAX as u64 {
            return Err(PeerStorageBudgetError::OutOfRange);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(self) -> u64 {
        self.0
    }
}
