use std::time::Duration;

use trogon_std::{NonZeroDuration, ZeroDuration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseTtl(NonZeroDuration);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LeaseTtlError {
    #[error("lease ttl must not be zero")]
    ZeroDuration,
    #[error("lease ttl must be whole seconds")]
    SubsecondPrecisionUnsupported,
}

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
mod tests;
