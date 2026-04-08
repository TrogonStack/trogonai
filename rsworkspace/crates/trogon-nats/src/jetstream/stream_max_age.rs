use std::time::Duration;

use trogon_std::{NonZeroDuration, ZeroDuration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMaxAge {
    NoExpiry,
    ExpireAfter(NonZeroDuration),
}

impl StreamMaxAge {
    pub fn from_secs(secs: u64) -> Result<Self, ZeroDuration> {
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
mod tests {
    use super::*;

    #[test]
    fn no_expiry_is_zero() {
        assert_eq!(Duration::from(StreamMaxAge::NoExpiry), Duration::ZERO);
    }

    #[test]
    fn expire_after() {
        let age = StreamMaxAge::from_secs(3600).unwrap();
        assert_eq!(Duration::from(age), Duration::from_secs(3600));
    }

    #[test]
    fn zero_rejected() {
        assert!(matches!(StreamMaxAge::from_secs(0), Err(ZeroDuration)));
    }
}
