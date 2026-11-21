use crate::ProtocolError;

#[derive(Clone)]
pub struct SignalNumber(i32);

impl SignalNumber {
    pub fn new(value: i32) -> Result<Self, ProtocolError> {
        if !(1..=64).contains(&value) {
            return Err(ProtocolError::InvalidSignal);
        }
        Ok(Self(value))
    }
    pub fn get(&self) -> i32 {
        self.0
    }
}
