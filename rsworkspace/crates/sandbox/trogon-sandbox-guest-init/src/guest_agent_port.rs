use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use crate::GuestAgentPortError;

/// The nonzero vsock port the guest agent listens on.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct GuestAgentPort(NonZeroU32);

impl GuestAgentPort {
    /// The port used when the kernel command line does not name one.
    pub const DEFAULT: Self = Self(NonZeroU32::MIN.saturating_add(8109));

    pub fn new(port: u32) -> Result<Self, GuestAgentPortError> {
        NonZeroU32::new(port).map(Self).ok_or(GuestAgentPortError::Zero)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl FromStr for GuestAgentPort {
    type Err = GuestAgentPortError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let port = value.parse::<u32>().map_err(|source| GuestAgentPortError::NotANumber {
            value: value.to_owned(),
            source,
        })?;
        Self::new(port)
    }
}

impl fmt::Display for GuestAgentPort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
