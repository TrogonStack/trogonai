/// An option combination or value the upstream SDK rejects before it starts the CLI. `Display` is the upstream
/// message of the dialect the options select (TypeScript `Error`, Python `ValueError`).
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct OptionError {
    message: String,
}

impl OptionError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}
