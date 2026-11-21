use crate::ProtocolError;

#[derive(Clone)]
pub struct ReleaseTimeout(u64);

impl ReleaseTimeout {
    pub fn new(value: u64) -> Result<Self, ProtocolError> {
        if value == 0 || value > crate::constants::MAX_RELEASE_TIMEOUT_MS {
            return Err(ProtocolError::InvalidDeadline);
        }
        Ok(Self(value))
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
