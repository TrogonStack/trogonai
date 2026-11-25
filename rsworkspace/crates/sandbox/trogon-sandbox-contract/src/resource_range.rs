use serde::{Deserialize, Serialize};

use crate::ResourceRangeError;
use crate::resource_range_wire::ResourceRangeWire;

/// Inclusive bounds a worker enforces for one sized resource.
///
/// Construction and deserialization reject a minimum above the maximum, so every range admits at least one
/// value.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ResourceRangeWire")]
pub struct ResourceRange {
    min: u32,
    max: u32,
}

impl ResourceRange {
    pub const fn new(min: u32, max: u32) -> Result<Self, ResourceRangeError> {
        if min > max {
            return Err(ResourceRangeError::Reversed { min, max });
        }
        Ok(Self { min, max })
    }

    pub const fn min(self) -> u32 {
        self.min
    }

    pub const fn max(self) -> u32 {
        self.max
    }

    pub const fn contains(self, value: u32) -> bool {
        self.min <= value && value <= self.max
    }
}

impl TryFrom<ResourceRangeWire> for ResourceRange {
    type Error = ResourceRangeError;

    fn try_from(wire: ResourceRangeWire) -> Result<Self, Self::Error> {
        Self::new(wire.min, wire.max)
    }
}
