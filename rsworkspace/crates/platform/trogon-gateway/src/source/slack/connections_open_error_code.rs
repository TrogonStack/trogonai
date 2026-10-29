use std::fmt;

use super::constants::PERMANENT_CONNECTIONS_OPEN_ERRORS;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionsOpenErrorCode(String);

impl ConnectionsOpenErrorCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn is_permanent(&self) -> bool {
        PERMANENT_CONNECTIONS_OPEN_ERRORS.contains(&self.0.as_str())
    }
}

impl fmt::Display for ConnectionsOpenErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
