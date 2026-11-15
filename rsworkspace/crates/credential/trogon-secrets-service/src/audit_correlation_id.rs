use std::fmt;

/// Joins the secrets service's audit record to the provider-facing one.
///
/// Opaque and compared whole, per ADR#0023 Decision 6.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AuditCorrelationId(String);

impl AuditCorrelationId {
    pub fn new(value: impl Into<String>) -> Result<Self, AuditCorrelationIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(AuditCorrelationIdError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AuditCorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AuditCorrelationIdError {
    #[error("audit correlation id must not be empty")]
    Empty,
}

#[cfg(test)]
mod tests;
