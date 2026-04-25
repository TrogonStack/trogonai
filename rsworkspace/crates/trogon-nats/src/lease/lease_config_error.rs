use std::time::Duration;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LeaseConfigError {
    EmptyBucket,
    InvalidBucketName(String),
    EmptyKey,
    InvalidKeyName(String),
    RenewIntervalNotLessThanTtl { renew_interval: Duration, ttl: Duration },
}

impl std::fmt::Display for LeaseConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBucket => write!(f, "lease bucket must not be empty"),
            Self::InvalidBucketName(bucket) => write!(
                f,
                "lease bucket '{bucket}' must contain only ASCII letters, digits, '-' or '_'"
            ),
            Self::EmptyKey => write!(f, "lease key must not be empty"),
            Self::InvalidKeyName(key) => write!(
                f,
                "lease key '{key}' must contain only ASCII letters, digits, '-', '_', '/', '=' or '.'"
            ),
            Self::RenewIntervalNotLessThanTtl { renew_interval, ttl } => write!(
                f,
                "lease renew interval {:?} must be less than ttl {:?}",
                renew_interval, ttl
            ),
        }
    }
}

impl std::error::Error for LeaseConfigError {}
