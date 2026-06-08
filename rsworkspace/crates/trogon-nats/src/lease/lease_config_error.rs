use std::time::Duration;

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum LeaseConfigError {
    #[error("lease bucket must not be empty")]
    EmptyBucket,
    #[error("lease bucket '{0}' must contain only ASCII letters, digits, '-' or '_'")]
    InvalidBucketName(String),
    #[error("lease key must not be empty")]
    EmptyKey,
    #[error("lease key '{0}' must contain only ASCII letters, digits, '-', '_', '/', '=' or '.'")]
    InvalidKeyName(String),
    #[error("lease renew interval {renew_interval:?} must be less than ttl {ttl:?}")]
    RenewIntervalNotLessThanTtl { renew_interval: Duration, ttl: Duration },
}
