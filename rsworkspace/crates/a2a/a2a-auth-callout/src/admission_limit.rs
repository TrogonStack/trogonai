use std::num::NonZeroUsize;

use tokio::sync::Semaphore;

use crate::constants::DEFAULT_MAX_IN_FLIGHT_ADMISSIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionLimit(NonZeroUsize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionLimitError {
    #[error("admission concurrency limit must be nonzero")]
    Zero,
    #[error("admission concurrency limit exceeds the supported permit range")]
    TooLarge,
}

impl AdmissionLimit {
    pub fn new(limit: usize) -> Result<Self, AdmissionLimitError> {
        let limit = NonZeroUsize::new(limit).ok_or(AdmissionLimitError::Zero)?;
        if limit.get() > Semaphore::MAX_PERMITS {
            return Err(AdmissionLimitError::TooLarge);
        }
        Ok(Self(limit))
    }

    pub(crate) fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for AdmissionLimit {
    fn default() -> Self {
        Self(DEFAULT_MAX_IN_FLIGHT_ADMISSIONS)
    }
}
