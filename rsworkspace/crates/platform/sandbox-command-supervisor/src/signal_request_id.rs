use crate::ProtocolError;

#[derive(Clone)]
pub struct SignalRequestId(u64);

impl SignalRequestId {
    pub fn new(value: u64) -> Result<Self, ProtocolError> {
        if value == 0 {
            return Err(ProtocolError::InvalidSignal);
        }
        Ok(Self(value))
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
