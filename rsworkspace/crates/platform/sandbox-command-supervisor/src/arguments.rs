use crate::ProtocolError;

#[derive(Clone)]
pub struct Arguments(Vec<String>);

impl Arguments {
    pub fn new(value: Vec<String>) -> Result<Self, ProtocolError> {
        if value.is_empty()
            || value.len() > 1024
            || value[0].is_empty()
            || value.iter().any(|arg| arg.as_bytes().contains(&0))
            || value.iter().map(String::len).sum::<usize>() > crate::constants::MAX_ARGUMENT_BYTES
        {
            return Err(ProtocolError::InvalidArguments);
        }
        Ok(Self(value))
    }
    pub fn into_inner(self) -> Vec<String> {
        self.0
    }
}
