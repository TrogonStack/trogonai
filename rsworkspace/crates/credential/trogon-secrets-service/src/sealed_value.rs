/// A credential value sealed to one requester's xkey.
///
/// Only the requester's private key opens it, so it may cross the bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedValue(Vec<u8>);

impl SealedValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self, SealedValueError> {
        if bytes.is_empty() {
            return Err(SealedValueError::Empty);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SealedValueError {
    #[error("sealed value must not be empty")]
    Empty,
}

#[cfg(test)]
mod tests;
