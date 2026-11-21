use crate::ProtocolError;

#[derive(Clone)]
pub struct CommandTimeout(u64);

impl CommandTimeout {
    pub fn new(value: u64) -> Result<Self, ProtocolError> {
        if value == 0 || value > crate::constants::MAX_COMMAND_TIMEOUT_MS {
            return Err(ProtocolError::InvalidDeadline);
        }
        Ok(Self(value))
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
