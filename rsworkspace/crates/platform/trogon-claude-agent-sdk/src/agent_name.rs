#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AgentName(String);

impl AgentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for AgentName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AgentName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
