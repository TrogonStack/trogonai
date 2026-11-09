#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AgentName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid agent name")]
pub struct AgentNameError;

impl AgentName {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentNameError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(AgentNameError);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
