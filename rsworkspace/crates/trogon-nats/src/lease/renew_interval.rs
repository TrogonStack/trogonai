use std::fmt;
use std::time::Duration;

use trogon_std::{NonZeroDuration, ZeroDuration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseRenewInterval(NonZeroDuration);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseRenewIntervalError {
    ZeroDuration,
}

impl fmt::Display for LeaseRenewIntervalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration => f.write_str("lease renew interval must not be zero"),
        }
    }
}

impl std::error::Error for LeaseRenewIntervalError {}

impl LeaseRenewInterval {
    pub fn new(renew_interval: NonZeroDuration) -> Self {
        Self(renew_interval)
    }

    pub fn from_secs(secs: u64) -> Result<Self, LeaseRenewIntervalError> {
        NonZeroDuration::from_secs(secs)
            .map(Self)
            .map_err(|_: ZeroDuration| LeaseRenewIntervalError::ZeroDuration)
    }

    pub fn from_millis(millis: u64) -> Result<Self, LeaseRenewIntervalError> {
        NonZeroDuration::from_millis(millis)
            .map(Self)
            .map_err(|_: ZeroDuration| LeaseRenewIntervalError::ZeroDuration)
    }

    pub fn as_duration(self) -> Duration {
        self.0.into()
    }
}

impl From<NonZeroDuration> for LeaseRenewInterval {
    fn from(value: NonZeroDuration) -> Self {
        Self(value)
    }
}

impl From<LeaseRenewInterval> for NonZeroDuration {
    fn from(value: LeaseRenewInterval) -> Self {
        value.0
    }
}

impl From<LeaseRenewInterval> for Duration {
    fn from(value: LeaseRenewInterval) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod tests;
