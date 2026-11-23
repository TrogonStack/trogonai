use serde::{Deserialize, Serialize};

/// Inclusive bounds a worker enforces for one sized resource.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceRange {
    pub min: u32,
    pub max: u32,
}

impl ResourceRange {
    pub fn contains(&self, value: u32) -> bool {
        (self.min..=self.max).contains(&value)
    }
}
