use serde::{Deserialize, Serialize};

use crate::RetentionTagLimitError;

/// How many snapshot tags a retention policy keeps.
///
/// It lies between [`RetentionTagLimit::MIN`] and [`RetentionTagLimit::MAX`]. Construction and
/// deserialization both enforce it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct RetentionTagLimit(u32);

impl RetentionTagLimit {
    /// Fewest tags a policy may keep.
    pub const MIN: u32 = 1;
    /// Most tags a policy may keep.
    pub const MAX: u32 = 10;

    pub fn new(value: u32) -> Result<Self, RetentionTagLimitError> {
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(RetentionTagLimitError::OutOfRange {
                min: Self::MIN,
                max: Self::MAX,
            });
        }
        Ok(Self(value))
    }

    pub fn get(&self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for RetentionTagLimit {
    type Error = RetentionTagLimitError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RetentionTagLimit> for u32 {
    fn from(value: RetentionTagLimit) -> Self {
        value.0
    }
}
