use crate::ProtocolError;

#[derive(Clone)]
pub struct ProtocolVersion(u32);

impl ProtocolVersion {
    pub fn new(value: u32) -> Result<Self, ProtocolError> {
        if value != crate::constants::VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        Ok(Self(value))
    }
    pub fn get(&self) -> u32 {
        self.0
    }
}
