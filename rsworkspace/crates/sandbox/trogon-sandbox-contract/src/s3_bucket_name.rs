use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::S3BucketNameError;

/// The name of an S3 bucket.
///
/// It holds 3 to [`S3BucketName::MAX_LEN`] bytes of lowercase ASCII letters, digits, `.`, or `-`, and starts and ends
/// with a letter or a digit. It serializes as a JSON string, and construction, parsing, and deserialization all
/// enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct S3BucketName(String);

impl S3BucketName {
    /// Shortest bucket name, in bytes.
    pub const MIN_LEN: usize = 3;
    /// Longest bucket name, in bytes.
    pub const MAX_LEN: usize = 63;

    pub fn new(value: impl Into<String>) -> Result<Self, S3BucketNameError> {
        let value = value.into();
        let edge = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        let valid = (Self::MIN_LEN..=Self::MAX_LEN).contains(&value.len())
            && value.as_bytes().first().is_some_and(edge)
            && value.as_bytes().last().is_some_and(edge)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(S3BucketNameError(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for S3BucketName {
    type Error = S3BucketNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for S3BucketName {
    type Err = S3BucketNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<S3BucketName> for String {
    fn from(name: S3BucketName) -> Self {
        name.0
    }
}

impl fmt::Display for S3BucketName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
