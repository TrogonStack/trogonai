use std::fmt;
use std::num::NonZeroU16;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::PortError;

/// A network port from 1 to 65535.
///
/// It serializes as a JSON number. Construction, parsing, and deserialization reject zero. Parsing from text also
/// rejects a sign, a leading zero, and anything that is not one to five ASCII digits.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Port(NonZeroU16);

impl Port {
    /// Port 443, which an inject host without a port stands for under scope version 2.
    pub(crate) const HTTPS: Self = Self(NonZeroU16::MIN.saturating_add(442));

    pub fn new(port: u16) -> Result<Self, PortError> {
        NonZeroU16::new(port)
            .map(Self)
            .ok_or_else(|| PortError(port.to_string()))
    }

    pub fn get(self) -> u16 {
        self.0.get()
    }
}

impl TryFrom<u16> for Port {
    type Error = PortError;

    fn try_from(port: u16) -> Result<Self, Self::Error> {
        Self::new(port)
    }
}

impl From<Port> for u16 {
    fn from(port: Port) -> Self {
        port.get()
    }
}

impl FromStr for Port {
    type Err = PortError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let well_formed =
            value.len() <= 5 && value.bytes().all(|byte| byte.is_ascii_digit()) && !value.starts_with('0');
        well_formed
            .then(|| value.parse::<u16>().ok())
            .flatten()
            .and_then(|port| Self::new(port).ok())
            .ok_or_else(|| PortError(value.to_string()))
    }
}

impl fmt::Display for Port {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
