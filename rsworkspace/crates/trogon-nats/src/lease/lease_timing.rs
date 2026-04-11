use std::time::Duration;

use super::{LeaseConfigError, LeaseRenewInterval, LeaseTtl};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LeaseTiming {
    ttl: LeaseTtl,
    renew_interval: LeaseRenewInterval,
}

impl LeaseTiming {
    pub fn new(
        ttl: LeaseTtl,
        renew_interval: LeaseRenewInterval,
    ) -> Result<Self, LeaseConfigError> {
        if renew_interval.as_duration() >= ttl.as_duration() {
            return Err(LeaseConfigError::RenewIntervalNotLessThanTtl {
                renew_interval: renew_interval.as_duration(),
                ttl: ttl.as_duration(),
            });
        }

        Ok(Self {
            ttl,
            renew_interval,
        })
    }

    pub fn ttl(self) -> Duration {
        self.ttl.as_duration()
    }

    pub fn renew_interval(self) -> Duration {
        self.renew_interval.as_duration()
    }
}
