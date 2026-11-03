#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSubjectPattern(String);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid server subscription pattern")]
pub struct ServerSubjectPatternError;

impl ServerSubjectPattern {
    pub fn new(input: &str) -> Result<Self, ServerSubjectPatternError> {
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
