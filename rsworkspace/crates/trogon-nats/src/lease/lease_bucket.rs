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
