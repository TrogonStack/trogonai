use std::fmt;

/// Name of the MCP server hosting a tool.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerNameError {
    #[error("server name must not be empty")]
    Empty,
}

impl ServerName {
    pub fn new(value: impl Into<String>) -> Result<Self, ServerNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ServerNameError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ServerName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert_eq!(ServerName::new("").unwrap_err(), ServerNameError::Empty);
    }

    #[test]
    fn accepts_non_empty() {
        assert_eq!(ServerName::new("web-tools").unwrap().as_str(), "web-tools");
    }
}
