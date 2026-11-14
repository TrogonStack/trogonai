use super::NativeKvCasError;
use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeKvExpectation {
    key: Option<NonZeroU64>,
    next: NonZeroU64,
}

impl NativeKvExpectation {
    /// Zero is an exact native absence/empty-stream comparison, never a wildcard.
    pub fn from_sequences(expected_key: u64, expected_global: u64) -> Result<Self, NativeKvCasError> {
        if expected_key > expected_global {
            return Err(NativeKvCasError::InvalidExpectation);
        }
        let next = NonZeroU64::MIN
            .checked_add(expected_global)
            .ok_or(NativeKvCasError::InvalidExpectation)?;
        Ok(Self {
            key: NonZeroU64::new(expected_key),
            next,
        })
    }

    pub fn key_sequence(self) -> u64 {
        self.key.map_or(0, NonZeroU64::get)
    }

    pub fn global_sequence(self) -> u64 {
        self.next.get() - 1
    }

    pub(crate) fn next_sequence(self) -> NonZeroU64 {
        self.next
    }
}
