use std::num::NonZeroUsize;

use tokio::sync::Semaphore;

use super::AdmissionLimitError;
use super::constants::DEFAULT_ADMISSION_LIMIT;

/// How many releases one member of the release queue group works on at once.
///
/// A call beyond it is answered `THROTTLED` without waiting, so a burst never
/// queues releases behind one another past their deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdmissionLimit(NonZeroUsize);

impl AdmissionLimit {
    pub fn new(limit: usize) -> Result<Self, AdmissionLimitError> {
        let limit = NonZeroUsize::new(limit).ok_or(AdmissionLimitError::Zero)?;
        if limit.get() > Semaphore::MAX_PERMITS {
            return Err(AdmissionLimitError::TooLarge);
        }
        Ok(Self(limit))
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for AdmissionLimit {
    fn default() -> Self {
        Self(DEFAULT_ADMISSION_LIMIT)
    }
}

#[cfg(test)]
mod tests;
