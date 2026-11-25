use crate::audit_name::{AuditName, AuditNameError};

/// The process instance that publishes records, carried in the
/// `Trogon-Audit-Writer` header and in `integrity.writer_id`.
///
/// It is a statement about the writer and never about who caused the act.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WriterId(AuditName);

impl WriterId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, AuditNameError> {
        AuditName::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for WriterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests;
