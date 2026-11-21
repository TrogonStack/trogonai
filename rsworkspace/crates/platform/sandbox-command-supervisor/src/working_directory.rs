use crate::ProtocolError;

#[derive(Clone)]
pub struct WorkingDirectory(String);

impl WorkingDirectory {
    pub fn new(value: String) -> Result<Self, ProtocolError> {
        if value.is_empty() || value.len() > 4096 || value.as_bytes().contains(&0) {
            return Err(ProtocolError::InvalidDirectory);
        }
        Ok(Self(value))
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}
