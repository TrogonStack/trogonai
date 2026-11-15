/// The audit trail did not acknowledge a record.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("audit record was not acknowledged: {message}")]
pub struct ReleaseAuditError {
    message: String,
}

impl ReleaseAuditError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(test)]
mod tests;
