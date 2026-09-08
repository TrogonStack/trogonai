//! Untrusted command-trigger text before validation.

#[cfg(test)]
mod tests;

/// Raw trigger text from config or another boundary. Convert once into
/// [`crate::CommandTrigger`].
#[derive(Debug, Clone)]
pub struct CommandTriggerInput(String);

impl CommandTriggerInput {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for CommandTriggerInput {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

impl From<&str> for CommandTriggerInput {
    fn from(raw: &str) -> Self {
        Self(raw.to_string())
    }
}
