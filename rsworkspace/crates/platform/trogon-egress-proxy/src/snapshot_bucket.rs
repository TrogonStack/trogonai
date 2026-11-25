use std::fmt;

use crate::constants::LIVE_POLICY_MAX_BUCKET_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBucket(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("snapshot bucket must be a bounded NATS bucket name")]
pub struct SnapshotBucketError;

impl SnapshotBucket {
    pub fn new(value: &str) -> Result<Self, SnapshotBucketError> {
        if value.is_empty()
            || value.len() > LIVE_POLICY_MAX_BUCKET_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(SnapshotBucketError);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SnapshotBucket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
