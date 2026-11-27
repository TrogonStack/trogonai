use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::TerminalArgumentError;

/// One argument of the command a terminal starts.
///
/// It must not contain NUL, which a process argument cannot carry. Construction, parsing, and deserialization all
/// enforce it.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TerminalArgument(String);

impl TerminalArgument {
    pub fn new(value: impl Into<String>) -> Result<Self, TerminalArgumentError> {
        let value = value.into();
        if value.contains('\0') {
            return Err(TerminalArgumentError::ContainsNul);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TerminalArgument {
    type Error = TerminalArgumentError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for TerminalArgument {
    type Err = TerminalArgumentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<TerminalArgument> for String {
    fn from(value: TerminalArgument) -> Self {
        value.0
    }
}

impl fmt::Display for TerminalArgument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
