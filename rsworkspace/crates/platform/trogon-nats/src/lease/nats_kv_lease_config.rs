use std::time::Duration;

use super::{LeaseBucket, LeaseConfigError, LeaseKey, LeaseRenewInterval, LeaseTiming, LeaseTtl};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NatsKvLeaseConfig {
    bucket: LeaseBucket,
    key: LeaseKey,
    timing: LeaseTiming,
}

impl NatsKvLeaseConfig {
    pub fn new(
        bucket: impl Into<String>,
        key: impl Into<String>,
        ttl: LeaseTtl,
        renew_interval: LeaseRenewInterval,
    ) -> Result<Self, LeaseConfigError> {
        let bucket = LeaseBucket::new(bucket)?;
        let key = LeaseKey::new(key)?;
        let timing = LeaseTiming::new(ttl, renew_interval)?;

        Ok(Self { bucket, key, timing })
    }

    pub fn bucket(&self) -> &LeaseBucket {
        &self.bucket
    }

    pub fn key(&self) -> &LeaseKey {
        &self.key
    }

    pub fn timing(&self) -> LeaseTiming {
        self.timing
    }

    pub fn ttl(&self) -> Duration {
        self.timing.ttl()
    }

    pub fn renew_interval(&self) -> Duration {
        self.timing.renew_interval()
    }
}
