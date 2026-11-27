use serde::{Deserialize, Serialize};

use crate::PidsLimitError;

/// A process ceiling a lease may ask for, from 1 to [`PidsLimit::MAX`].
///
/// It serializes as a JSON number, and construction and deserialization both enforce the range.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct PidsLimit(u32);

impl PidsLimit {
    /// Largest process ceiling a lease may ask for.
    pub const MAX: u32 = 65_536;

    pub fn new(limit: u32) -> Result<Self, PidsLimitError> {
        if (1..=Self::MAX).contains(&limit) {
            Ok(Self(limit))
        } else {
            Err(PidsLimitError {
                max: Self::MAX,
                got: limit,
            })
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for PidsLimit {
    type Error = PidsLimitError;

    fn try_from(limit: u32) -> Result<Self, Self::Error> {
        Self::new(limit)
    }
}

impl From<PidsLimit> for u32 {
    fn from(limit: PidsLimit) -> Self {
        limit.0
    }
}
