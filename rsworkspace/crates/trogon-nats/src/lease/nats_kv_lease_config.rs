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

#[cfg(test)]
mod tests {
    use super::*;

    fn ttl(secs: u64) -> LeaseTtl { LeaseTtl::from_secs(secs).unwrap() }
    fn interval(secs: u64) -> LeaseRenewInterval { LeaseRenewInterval::from_secs(secs).unwrap() }

    #[test]
    fn valid_config_accessors() {
        let cfg = NatsKvLeaseConfig::new("my-bucket", "my/key", ttl(10), interval(3)).unwrap();
        assert_eq!(cfg.bucket().as_str(), "my-bucket");
        assert_eq!(cfg.key().as_str(), "my/key");
        assert_eq!(cfg.ttl(), Duration::from_secs(10));
        assert_eq!(cfg.renew_interval(), Duration::from_secs(3));
    }

    #[test]
    fn invalid_bucket_propagates_error() {
        let err = NatsKvLeaseConfig::new("bad.bucket", "key", ttl(10), interval(3)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::InvalidBucketName(_)));
    }

    #[test]
    fn invalid_key_propagates_error() {
        let err = NatsKvLeaseConfig::new("bucket", "bad key", ttl(10), interval(3)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::InvalidKeyName(_)));
    }

    #[test]
    fn bad_timing_propagates_error() {
        let err = NatsKvLeaseConfig::new("bucket", "key", ttl(5), interval(5)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::RenewIntervalNotLessThanTtl { .. }));
    }

    #[test]
    fn empty_bucket_propagates_error() {
        let err = NatsKvLeaseConfig::new("", "key", ttl(10), interval(3)).unwrap_err();
        assert!(matches!(err, LeaseConfigError::EmptyBucket));
    }
}
