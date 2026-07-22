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
}

impl From<NonZeroDuration> for Duration {
    fn from(d: NonZeroDuration) -> Self {
        d.0
    }
}

#[cfg(test)]
mod tests;
