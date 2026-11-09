use std::num::NonZeroUsize;

use crate::constants::{DEFAULT_MAX_CONNECTIONS, MAX_CONNECTION_LIMIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionLimit(NonZeroUsize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionLimitError {
    #[error("connection limit must be nonzero")]
    Zero,
    #[error("connection limit cannot exceed {MAX_CONNECTION_LIMIT}")]
    TooLarge,
}

impl ConnectionLimit {
    pub fn new(limit: usize) -> Result<Self, ConnectionLimitError> {
        let limit = NonZeroUsize::new(limit).ok_or(ConnectionLimitError::Zero)?;
        if limit.get() > MAX_CONNECTION_LIMIT {
            return Err(ConnectionLimitError::TooLarge);
        }
        Ok(Self(limit))
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for ConnectionLimit {
    fn default() -> Self {
        Self(DEFAULT_MAX_CONNECTIONS)
    }
}
