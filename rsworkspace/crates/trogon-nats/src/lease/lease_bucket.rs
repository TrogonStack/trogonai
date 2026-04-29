use super::lease_config_error::LeaseConfigError;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LeaseBucket(String);

impl LeaseBucket {
    pub fn new(bucket: impl Into<String>) -> Result<Self, LeaseConfigError> {
        let bucket = bucket.into();
        if bucket.is_empty() {
            return Err(LeaseConfigError::EmptyBucket);
        }
        if !Self::is_valid_name(&bucket) {
            return Err(LeaseConfigError::InvalidBucketName(bucket));
        }
        Ok(Self(bucket))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid_name(bucket: &str) -> bool {
        bucket.chars().all(Self::is_valid_char)
    }

    fn is_valid_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_alphanumeric_bucket() {
        let b = LeaseBucket::new("my-bucket_01").unwrap();
        assert_eq!(b.as_str(), "my-bucket_01");
    }

    #[test]
    fn empty_bucket_is_rejected() {
        assert!(matches!(LeaseBucket::new(""), Err(LeaseConfigError::EmptyBucket)));
    }

    #[test]
    fn bucket_with_dot_is_rejected() {
        // dots are allowed in keys but NOT in buckets
        assert!(matches!(LeaseBucket::new("my.bucket"), Err(LeaseConfigError::InvalidBucketName(_))));
    }

    #[test]
    fn bucket_with_slash_is_rejected() {
        assert!(matches!(LeaseBucket::new("a/b"), Err(LeaseConfigError::InvalidBucketName(_))));
    }

    #[test]
    fn bucket_with_space_is_rejected() {
        assert!(matches!(LeaseBucket::new("bad bucket"), Err(LeaseConfigError::InvalidBucketName(_))));
    }
}
