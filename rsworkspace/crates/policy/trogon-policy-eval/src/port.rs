/// Why a number cannot be a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// Port zero is not a destination.
    #[error("port cannot be zero")]
    Zero,
    /// The number does not fit in 16 bits.
    #[error("port {0} is above 65535")]
    OutOfRange(u32),
}

/// A TCP port, from 1 to 65535.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Port(u16);

impl Port {
    /// Creates a port, refusing zero.
    pub const fn new(value: u16) -> Result<Self, PortError> {
        if value == 0 {
            return Err(PortError::Zero);
        }
        Ok(Self(value))
    }

    /// Returns the port number.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u32> for Port {
    type Error = PortError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        let narrowed = u16::try_from(value).map_err(|_| PortError::OutOfRange(value))?;
        Self::new(narrowed)
    }
}
