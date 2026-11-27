use std::fmt;

use serde::{Deserialize, Serialize};

use crate::TerminalDimensionError;

/// One side of a terminal's size, in character cells.
///
/// It holds 1 to [`TerminalDimension::MAX`] and serializes as a JSON number. Construction and
/// deserialization both enforce the range.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct TerminalDimension(u16);

impl TerminalDimension {
    /// Largest accepted dimension, in cells.
    pub const MAX: u16 = 1000;

    pub fn new(value: u16) -> Result<Self, TerminalDimensionError> {
        if (1..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(TerminalDimensionError::OutOfRange { max: Self::MAX })
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for TerminalDimension {
    type Error = TerminalDimensionError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TerminalDimension> for u16 {
    fn from(value: TerminalDimension) -> Self {
        value.0
    }
}

impl fmt::Display for TerminalDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
