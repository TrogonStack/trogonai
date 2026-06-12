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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_empty_bucket() {
        assert_eq!(
            LeaseConfigError::EmptyBucket.to_string(),
            "lease bucket must not be empty"
        );
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
            ttl: Duration::from_secs(5),
        }
        .to_string();
        assert!(
            msg.contains("10s") || msg.contains("10"),
            "expected renew_interval in message, got: {msg}"
        );
        assert!(
            msg.contains("5s") || msg.contains("5"),
            "expected ttl in message, got: {msg}"
        );
    }
}
