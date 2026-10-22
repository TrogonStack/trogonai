use crate::Error;

#[derive(Debug, Clone)]
pub struct EnvironmentVariable(String);
impl EnvironmentVariable {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty()
            || value.len() > 200
            || !value
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            || value.as_bytes()[0].is_ascii_digit()
        {
            return Err(Error::Configuration);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
