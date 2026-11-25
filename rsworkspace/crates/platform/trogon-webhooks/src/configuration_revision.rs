use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationRevision(NonZeroU64);

#[derive(Debug, thiserror::Error)]
#[error("configuration revision must be nonzero")]
pub struct ConfigurationRevisionError;

impl ConfigurationRevision {
    pub fn new(value: u64) -> Result<Self, ConfigurationRevisionError> {
        NonZeroU64::new(value).map(Self).ok_or(ConfigurationRevisionError)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}
