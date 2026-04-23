use std::fmt;
use std::time::Duration;

use trogon_std::{NonZeroDuration, ZeroDuration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseTtl(NonZeroDuration);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseTtlError {
    ZeroDuration,
    SubsecondPrecisionUnsupported,
}

impl fmt::Display for LeaseTtlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration => f.write_str("lease ttl must not be zero"),
            Self::SubsecondPrecisionUnsupported => f.write_str("lease ttl must be whole seconds"),
        }
    }
}

impl std::error::Error for LeaseTtlError {}

impl LeaseTtl {
    pub fn new(ttl: NonZeroDuration) -> Result<Self, LeaseTtlError> {
        let ttl_duration: Duration = ttl.into();
        if ttl_duration.subsec_nanos() != 0 {
            return Err(LeaseTtlError::SubsecondPrecisionUnsupported);
        }
        Ok(Self(ttl))
    }

    pub fn from_secs(secs: u64) -> Result<Self, LeaseTtlError> {
        let ttl = NonZeroDuration::from_secs(secs).map_err(|_: ZeroDuration| LeaseTtlError::ZeroDuration)?;
        Self::new(ttl)
    }

    pub fn from_millis(millis: u64) -> Result<Self, LeaseTtlError> {
        let ttl = NonZeroDuration::from_millis(millis).map_err(|_: ZeroDuration| LeaseTtlError::ZeroDuration)?;
        Self::new(ttl)
    }

    pub fn as_duration(self) -> Duration {
        self.0.into()
    }
}

impl TryFrom<NonZeroDuration> for LeaseTtl {
    type Error = LeaseTtlError;

    fn try_from(value: NonZeroDuration) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<LeaseTtl> for NonZeroDuration {
    fn from(value: LeaseTtl) -> Self {
        value.0
    }
}

impl From<LeaseTtl> for Duration {
    fn from(value: LeaseTtl) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_secs_accepts_whole_seconds() {
        let ttl = LeaseTtl::from_secs(2).unwrap();
        assert_eq!(ttl.as_duration(), Duration::from_secs(2));
    }

    #[test]
    fn from_secs_rejects_zero() {
        let err = LeaseTtl::from_secs(0).unwrap_err();
        assert_eq!(err, LeaseTtlError::ZeroDuration);
    }

    #[test]
    fn from_millis_rejects_subsecond_values() {
        let err = LeaseTtl::from_millis(500).unwrap_err();
        assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
    }

    #[test]
    fn from_millis_rejects_non_whole_seconds() {
        let err = LeaseTtl::from_millis(1500).unwrap_err();
        assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
    }

    #[test]
    fn from_millis_accepts_exact_whole_seconds() {
        let ttl = LeaseTtl::from_millis(2000).unwrap();
        assert_eq!(ttl.as_duration(), Duration::from_secs(2));
    }

    #[test]
    fn from_millis_rejects_zero() {
        let err = LeaseTtl::from_millis(0).unwrap_err();
        assert_eq!(err, LeaseTtlError::ZeroDuration);
    }

    #[test]
    fn try_from_non_zero_duration_rejects_subsecond_values() {
        let value = NonZeroDuration::from_millis(250).unwrap();
        let err = LeaseTtl::try_from(value).unwrap_err();
        assert_eq!(err, LeaseTtlError::SubsecondPrecisionUnsupported);
    }

    #[test]
    fn conversions_preserve_whole_second_value() {
        let ttl = LeaseTtl::from_secs(5).unwrap();
        let non_zero: NonZeroDuration = ttl.into();
        let duration: Duration = ttl.into();

        assert_eq!(Duration::from(non_zero), Duration::from_secs(5));
        assert_eq!(duration, Duration::from_secs(5));
    }

    #[test]
    fn display_messages_are_actionable() {
        assert_eq!(LeaseTtlError::ZeroDuration.to_string(), "lease ttl must not be zero");
        assert_eq!(
            LeaseTtlError::SubsecondPrecisionUnsupported.to_string(),
            "lease ttl must be whole seconds"
        );
    }
}
