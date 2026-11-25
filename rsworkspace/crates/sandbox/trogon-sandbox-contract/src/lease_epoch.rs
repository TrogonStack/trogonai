use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::LeaseEpochError;

/// The order of a lease's worker assignments.
///
/// It starts at [`LeaseEpoch::FIRST`] and grows whenever the worker and attempt that own the lease change, so a
/// worker can tell a newer assignment from a delayed message about an older one. It is never zero; construction
/// and deserialization both reject zero.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseEpoch(NonZeroU64);

impl LeaseEpoch {
    /// The epoch of a lease's first assignment.
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    pub fn new(value: u64) -> Result<Self, LeaseEpochError> {
        NonZeroU64::new(value).map(Self).ok_or(LeaseEpochError::Zero)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl From<NonZeroU64> for LeaseEpoch {
    fn from(value: NonZeroU64) -> Self {
        Self(value)
    }
}

impl TryFrom<u64> for LeaseEpoch {
    type Error = LeaseEpochError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<LeaseEpoch> for u64 {
    fn from(value: LeaseEpoch) -> Self {
        value.get()
    }
}

impl fmt::Display for LeaseEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
