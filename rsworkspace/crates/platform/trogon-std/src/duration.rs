use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonZeroDuration(Duration);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("duration must not be zero")]
pub struct ZeroDurationError;

impl NonZeroDuration {
    pub fn from_secs(secs: u64) -> Result<Self, ZeroDurationError> {
        if secs == 0 {
            return Err(ZeroDurationError);
        }
        Ok(Self(Duration::from_secs(secs)))
    }

    pub fn from_millis(millis: u64) -> Result<Self, ZeroDurationError> {
        if millis == 0 {
            return Err(ZeroDurationError);
        }
        Ok(Self(Duration::from_millis(millis)))
    }

    /// Construct from a compile-time constant number of seconds, for `const`
    /// items. A zero `secs` is a compile error when the constant is evaluated.
    pub const fn from_secs_const(secs: u64) -> Self {
        assert!(secs != 0, "duration must not be zero");
        Self(Duration::from_secs(secs))
    }
}

impl From<NonZeroDuration> for Duration {
    fn from(d: NonZeroDuration) -> Self {
        d.0
    }
}

#[cfg(test)]
mod tests;
