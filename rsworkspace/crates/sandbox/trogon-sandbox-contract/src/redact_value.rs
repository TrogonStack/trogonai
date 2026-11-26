use std::fmt;

use serde::{Deserialize, Serialize};

/// A literal a worker replaces with `[REDACTED]` in everything it returns for a command.
///
/// Any string is accepted. `Debug` never shows the value and there is no `Display`, so logging a request cannot leak
/// it. Serialization still carries it as a JSON string, because it has to cross from the caller to the worker that
/// filters the output, so a request carrying it must never be logged or stored as a record.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RedactValue(String);

impl RedactValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Keep it out of logs.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for RedactValue {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<RedactValue> for String {
    fn from(value: RedactValue) -> Self {
        value.0
    }
}

impl fmt::Debug for RedactValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RedactValue([REDACTED])")
    }
}
