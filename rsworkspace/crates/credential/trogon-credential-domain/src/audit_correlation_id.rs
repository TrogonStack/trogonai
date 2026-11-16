use std::fmt;
use std::sync::Arc;

use super::constants::MAX_AUDIT_CORRELATION_ID_LEN;

/// Joins the secrets service's audit record to the provider-facing one.
///
/// Opaque and compared whole, per ADR#0023 Decision 6. It holds only the
/// characters an audit record name holds, so the secrets service's record and
/// the provider-facing record each carry it whole. The id is also threaded into
/// the secret store's request so the store's own audit log carries it, which
/// can mean an HTTP header; each of these characters travels there unescaped,
/// and none of them can fold a second header into the request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AuditCorrelationId(Arc<str>);

impl AuditCorrelationId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, AuditCorrelationIdError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(AuditCorrelationIdError::Empty);
        }
        let char_count = value.chars().count();
        if char_count > MAX_AUDIT_CORRELATION_ID_LEN {
            return Err(AuditCorrelationIdError::TooLong(char_count));
        }
        if let Some(ch) = value.chars().find(|ch| !is_audit_correlation_id_char(*ch)) {
            return Err(AuditCorrelationIdError::InvalidCharacter(ch));
        }
        Ok(Self(Arc::from(value)))
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
    #[error("audit correlation id exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("audit correlation id contains invalid character '{0}'")]
    InvalidCharacter(char),
}

/// ASCII alphanumerics and `._:/@+~[]-`, the characters an audit record name
/// holds.
fn is_audit_correlation_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '/' | '@' | '+' | '~' | '[' | ']' | '-')
}

#[cfg(test)]
mod tests;
