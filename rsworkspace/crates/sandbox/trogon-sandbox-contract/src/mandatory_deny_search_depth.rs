use serde::{Deserialize, Serialize};

use crate::MandatoryDenySearchDepthError;

/// How many levels below a writable root the Linux scan looks for protected names.
///
/// A protected file is found down to this level, and a protected directory, or the hooks and config of a
/// repository, one level higher. The depth is from [`MandatoryDenySearchDepth::MIN`] to
/// [`MandatoryDenySearchDepth::MAX`]. It serializes as a JSON number, and construction and deserialization both
/// enforce the range.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct MandatoryDenySearchDepth(u8);

impl MandatoryDenySearchDepth {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 10;

    /// The depth that applies when none is configured.
    pub const DEFAULT: Self = Self(3);

    pub fn new(levels: u8) -> Result<Self, MandatoryDenySearchDepthError> {
        if (Self::MIN..=Self::MAX).contains(&levels) {
            Ok(Self(levels))
        } else {
            Err(MandatoryDenySearchDepthError {
                min: Self::MIN,
                max: Self::MAX,
                got: levels,
            })
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for MandatoryDenySearchDepth {
    type Error = MandatoryDenySearchDepthError;

    fn try_from(levels: u8) -> Result<Self, Self::Error> {
        Self::new(levels)
    }
}

impl From<MandatoryDenySearchDepth> for u8 {
    fn from(depth: MandatoryDenySearchDepth) -> Self {
        depth.0
    }
}
