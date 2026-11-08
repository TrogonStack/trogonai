use std::time::Duration;

/// Deadline for the first imported non-unary reply, independent of the event pump.
/// Bounds resource retention to one hour even when an agent never responds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonUnaryRelayTimeout(Duration);

impl NonUnaryRelayTimeout {
    /// Accepts 1 through 3600 seconds; zero cannot disable relay cleanup.
    pub fn new(seconds: u64) -> Result<Self, NonUnaryRelayTimeoutError> {
        if !(1..=3600).contains(&seconds) {
            return Err(NonUnaryRelayTimeoutError::OutOfRange);
        }
        Ok(Self(Duration::from_secs(seconds)))
    }

    pub fn duration(self) -> Duration {
        self.0
    }
}

impl Default for NonUnaryRelayTimeout {
    fn default() -> Self {
        Self(Duration::from_secs(300))
    }
}

impl std::str::FromStr for NonUnaryRelayTimeout {
    type Err = NonUnaryRelayTimeoutError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::new(raw.trim().parse().map_err(NonUnaryRelayTimeoutError::Parse)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NonUnaryRelayTimeoutError {
    #[error("non-unary relay timeout must be an integer number of seconds")]
    Parse(#[source] std::num::ParseIntError),
    #[error("non-unary relay timeout must be between 1 and 3600 seconds")]
    OutOfRange,
}
