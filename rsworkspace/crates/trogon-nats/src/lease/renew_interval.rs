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
mod tests {
    use super::*;

    #[test]
    fn from_secs_accepts_non_zero() {
        let interval = LeaseRenewInterval::from_secs(3).unwrap();
        assert_eq!(interval.as_duration(), Duration::from_secs(3));
    }

    #[test]
    fn from_secs_rejects_zero() {
        let err = LeaseRenewInterval::from_secs(0).unwrap_err();
        assert_eq!(err, LeaseRenewIntervalError::ZeroDuration);
    }

    #[test]
    fn from_millis_accepts_non_zero() {
        let interval = LeaseRenewInterval::from_millis(250).unwrap();
        assert_eq!(interval.as_duration(), Duration::from_millis(250));
    }

    #[test]
    fn from_millis_rejects_zero() {
        let err = LeaseRenewInterval::from_millis(0).unwrap_err();
        assert_eq!(err, LeaseRenewIntervalError::ZeroDuration);
    }

    #[test]
    fn new_and_conversions_preserve_value() {
        let non_zero = NonZeroDuration::from_secs(4).unwrap();
        let interval = LeaseRenewInterval::new(non_zero);
        let back_to_non_zero: NonZeroDuration = interval.into();
        let as_duration: Duration = interval.into();

        assert_eq!(Duration::from(back_to_non_zero), Duration::from_secs(4));
        assert_eq!(as_duration, Duration::from_secs(4));
    }

    #[test]
    fn from_non_zero_duration_constructor_preserves_value() {
        let interval = LeaseRenewInterval::from(NonZeroDuration::from_secs(6).unwrap());
        assert_eq!(interval.as_duration(), Duration::from_secs(6));
    }

    #[test]
    fn display_message_is_actionable() {
        assert_eq!(
            LeaseRenewIntervalError::ZeroDuration.to_string(),
            "lease renew interval must not be zero"
        );
    }
}
