use std::time::Duration;

use super::{LeaseConfigError, LeaseRenewInterval, LeaseTtl};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LeaseTiming {
    ttl: LeaseTtl,
    renew_interval: LeaseRenewInterval,
}

impl LeaseTiming {
    pub fn new(ttl: LeaseTtl, renew_interval: LeaseRenewInterval) -> Result<Self, LeaseConfigError> {
        if renew_interval.as_duration() >= ttl.as_duration() {
            return Err(LeaseConfigError::RenewIntervalNotLessThanTtl {
                renew_interval: renew_interval.as_duration(),
                ttl: ttl.as_duration(),
            });
        }

        Ok(Self { ttl, renew_interval })
    }

    pub fn ttl(self) -> Duration {
        self.ttl.as_duration()
    }

    pub fn renew_interval(self) -> Duration {
        self.renew_interval.as_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ttl(secs: u64) -> LeaseTtl { LeaseTtl::from_secs(secs).unwrap() }
    fn interval(secs: u64) -> LeaseRenewInterval { LeaseRenewInterval::from_secs(secs).unwrap() }

    #[test]
    fn valid_timing_interval_less_than_ttl() {
        let t = LeaseTiming::new(ttl(10), interval(3)).unwrap();
        assert_eq!(t.ttl(), Duration::from_secs(10));
        assert_eq!(t.renew_interval(), Duration::from_secs(3));
    }

    #[test]
    fn equal_interval_and_ttl_is_rejected() {
        let err = LeaseTiming::new(ttl(5), interval(5)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::RenewIntervalNotLessThanTtl { .. }));
    }

    #[test]
    fn interval_greater_than_ttl_is_rejected() {
        let err = LeaseTiming::new(ttl(3), interval(10)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::RenewIntervalNotLessThanTtl { .. }));
    }
}
