use std::time::Duration;

use trogon_std::{NonZeroDuration, ZeroDurationError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMaxAge {
    NoExpiry,
    ExpireAfter(NonZeroDuration),
}

impl StreamMaxAge {
    pub fn from_secs(secs: u64) -> Result<Self, ZeroDurationError> {
        Ok(Self::ExpireAfter(NonZeroDuration::from_secs(secs)?))
    }
}

impl From<StreamMaxAge> for Duration {
    fn from(age: StreamMaxAge) -> Self {
        match age {
            StreamMaxAge::NoExpiry => Duration::ZERO,
            StreamMaxAge::ExpireAfter(d) => d.into(),
        }
    }
}

#[cfg(test)]
mod tests;
