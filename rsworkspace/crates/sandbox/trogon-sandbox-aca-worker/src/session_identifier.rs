use std::fmt;

use uuid::Uuid;

/// The identifier of one Dynamic Sessions session.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SessionIdentifier(String);

impl SessionIdentifier {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// A fresh identifier no other session of this worker has.
    pub fn generate() -> Self {
        Self(format!("aca-{}", Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionIdentifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
