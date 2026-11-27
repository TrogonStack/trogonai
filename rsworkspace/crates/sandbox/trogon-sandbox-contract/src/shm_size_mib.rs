use serde::{Deserialize, Serialize};

use crate::ShmSizeMibError;

/// A `/dev/shm` size in MiB a lease may ask for, from 1 to [`ShmSizeMib::MAX`].
///
/// It serializes as a JSON number, and construction and deserialization both enforce the range.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct ShmSizeMib(u32);

impl ShmSizeMib {
    /// Largest `/dev/shm` a lease may ask for, in MiB.
    pub const MAX: u32 = 8_192;

    pub fn new(mebibytes: u32) -> Result<Self, ShmSizeMibError> {
        if (1..=Self::MAX).contains(&mebibytes) {
            Ok(Self(mebibytes))
        } else {
            Err(ShmSizeMibError {
                max: Self::MAX,
                got: mebibytes,
            })
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for ShmSizeMib {
    type Error = ShmSizeMibError;

    fn try_from(mebibytes: u32) -> Result<Self, Self::Error> {
        Self::new(mebibytes)
    }
}

impl From<ShmSizeMib> for u32 {
    fn from(size: ShmSizeMib) -> Self {
        size.0
    }
}
