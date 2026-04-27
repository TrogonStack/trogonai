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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_empty_bucket() {
        assert_eq!(LeaseConfigError::EmptyBucket.to_string(), "lease bucket must not be empty");
    }

    #[test]
    fn display_invalid_bucket_name_includes_name() {
        let msg = LeaseConfigError::InvalidBucketName("my.bucket".into()).to_string();
        assert!(msg.contains("my.bucket"), "expected bucket name in message, got: {msg}");
    }

    #[test]
    fn display_empty_key() {
        assert_eq!(LeaseConfigError::EmptyKey.to_string(), "lease key must not be empty");
    }

    #[test]
    fn display_invalid_key_name_includes_name() {
        let msg = LeaseConfigError::InvalidKeyName("bad key".into()).to_string();
        assert!(msg.contains("bad key"), "expected key name in message, got: {msg}");
    }

    #[test]
    fn display_renew_interval_not_less_than_ttl_includes_durations() {
        let msg = LeaseConfigError::RenewIntervalNotLessThanTtl {
            renew_interval: Duration::from_secs(10),
            ttl:            Duration::from_secs(5),
        }.to_string();
        assert!(msg.contains("10s") || msg.contains("10"), "expected renew_interval in message, got: {msg}");
        assert!(msg.contains("5s") || msg.contains("5"), "expected ttl in message, got: {msg}");
    }
}
