use std::time::SystemTime;

use super::SealedValue;
use super::constants::UNBUNDLED_GENERATION;

/// A released credential value, sealed and bounded in time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedRelease {
    sealed_value: SealedValue,
    valid_until: SystemTime,
}

impl SealedRelease {
    pub fn new(sealed_value: SealedValue, valid_until: SystemTime) -> Self {
        Self {
            sealed_value,
            valid_until,
        }
    }

    pub fn sealed_value(&self) -> &SealedValue {
        &self.sealed_value
    }

    /// The generation the value came from, which a single value does not have.
    pub fn bundle_generation(&self) -> u64 {
        UNBUNDLED_GENERATION
    }

    /// When the holder must drop the value.
    pub fn valid_until(&self) -> SystemTime {
        self.valid_until
    }
}

#[cfg(test)]
mod tests;
