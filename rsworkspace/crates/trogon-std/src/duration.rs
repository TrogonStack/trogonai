use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonZeroDuration(Duration);

#[derive(Debug, PartialEq, Eq)]
pub struct ZeroDuration;

impl fmt::Display for ZeroDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duration must not be zero")
    }
}

impl std::error::Error for ZeroDuration {}

impl NonZeroDuration {
    pub fn from_secs(secs: u64) -> Result<Self, ZeroDuration> {
        if secs == 0 {
            return Err(ZeroDuration);
        }
        Ok(Self(Duration::from_secs(secs)))
    }

    pub fn from_millis(millis: u64) -> Result<Self, ZeroDuration> {
        if millis == 0 {
            return Err(ZeroDuration);
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
