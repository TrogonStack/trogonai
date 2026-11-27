use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::TunnelCapError;

/// How many TLS-terminated tunnels may be open at once.
///
/// It is from 1 to [`TunnelCap::MAX`] and serializes as a JSON number. Construction and deserialization reject zero
/// and anything above the maximum.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct TunnelCap(NonZeroU32);

impl TunnelCap {
    /// The largest cap the wire accepts.
    pub const MAX: u32 = 65_536;

    pub fn new(cap: u32) -> Result<Self, TunnelCapError> {
        NonZeroU32::new(cap)
            .filter(|cap| cap.get() <= Self::MAX)
            .map(Self)
            .ok_or(TunnelCapError::OutOfRange { max: Self::MAX })
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for TunnelCap {
    type Error = TunnelCapError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TunnelCap> for u32 {
    fn from(value: TunnelCap) -> Self {
        value.get()
    }
}
