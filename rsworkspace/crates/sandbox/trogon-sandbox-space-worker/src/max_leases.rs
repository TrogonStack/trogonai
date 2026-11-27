use crate::MaxLeasesError;

/// How many leases one worker holds at once, from 1 to [`MaxLeases::LIMIT`].
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MaxLeases(u32);

impl MaxLeases {
    /// The most leases a worker may be configured for.
    pub const LIMIT: u32 = 128;

    /// Parses a lease capacity.
    pub fn parse(value: u32) -> Result<Self, MaxLeasesError> {
        if (1..=Self::LIMIT).contains(&value) {
            Ok(Self(value))
        } else {
            Err(MaxLeasesError { limit: Self::LIMIT })
        }
    }

    /// The capacity.
    pub fn get(self) -> u32 {
        self.0
    }
}
