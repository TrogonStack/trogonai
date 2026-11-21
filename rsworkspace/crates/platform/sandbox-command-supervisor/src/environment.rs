use crate::ProtocolError;

#[derive(Clone)]
pub struct Environment(std::collections::BTreeMap<String, String>);

impl Environment {
    pub fn new(value: std::collections::BTreeMap<String, String>) -> Result<Self, ProtocolError> {
        if value.len() > 1024
            || value.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 8192
                    || key.contains('=')
                    || key.as_bytes().contains(&0)
                    || value.as_bytes().contains(&0)
            })
            || value
                .iter()
                .map(|(key, value)| key.len() + value.len() + 2)
                .sum::<usize>()
                > crate::constants::MAX_ENV_BYTES
        {
            return Err(ProtocolError::InvalidEnvironment);
        }
        Ok(Self(value))
    }
    pub fn into_inner(self) -> std::collections::BTreeMap<String, String> {
        self.0
    }
}
