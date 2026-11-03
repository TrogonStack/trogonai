use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerRequestTimeout(Duration);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerRequestTimeoutError {
    #[error("peer request timeout must be positive")]
    Zero,
    #[error("peer request timeout exceeds its two-second bound")]
    TooLong,
}

impl PeerRequestTimeout {
    pub const MAX: Duration = Duration::from_secs(2);

    pub fn new(timeout: Duration) -> Result<Self, PeerRequestTimeoutError> {
        Ok(Self(timeout))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}
