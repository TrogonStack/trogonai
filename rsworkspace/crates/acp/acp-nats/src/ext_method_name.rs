//! NATS-safe extension method name value object.
//!
//! Method names are embedded in subjects: `{prefix}.v1.global.agent.ext.{method}`.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! rejects `*`, `>`, whitespace; allows dotted namespaces (e.g. `vendor.operation`) but rejects
//! malformed dots (consecutive, leading, trailing). Validity is guaranteed at construction.
//!
//! The name is carried exactly as the ACP SDK hands it over, see [`ext_protocol_method`].

use std::borrow::Cow;

use trogon_nats::DottedNatsToken;
use trogon_nats::SubjectTokenViolationError;

/// Error returned when [`ExtMethodName`] validation fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExtMethodNameError {
    #[error("method must not be empty")]
    Empty,
    #[error("method contains invalid character: {0:?}")]
    InvalidCharacter(char),
    #[error("method is too long: {0} bytes (max 128)")]
    TooLong(usize),
}

impl From<SubjectTokenViolationError> for ExtMethodNameError {
    fn from(v: SubjectTokenViolationError) -> Self {
        match v {
            SubjectTokenViolationError::Empty => Self::Empty,
            SubjectTokenViolationError::InvalidCharacter(ch) => Self::InvalidCharacter(ch),
            SubjectTokenViolationError::TooLong(len) => Self::TooLong(len),
        }
    }
}

/// NATS-safe extension method name. Guarantees validity at construction—invalid instances are unrepresentable.
///
/// Rejects empty, too-long, wildcard, whitespace, and malformed dotted names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExtMethodName(DottedNatsToken);

impl ExtMethodName {
    pub fn new(method: impl AsRef<str>) -> Result<Self, ExtMethodNameError> {
        DottedNatsToken::new(method).map(Self).map_err(Into::into)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The ACP JSON-RPC method for this extension, see [`ext_protocol_method`].
    pub fn protocol_method(&self) -> Cow<'_, str> {
        ext_protocol_method(self.as_str())
    }
}

/// The ACP JSON-RPC method for an extension name: the name itself when it
/// already starts with `_`, otherwise the name with `_` in front.
///
/// The SDK keeps the `_` in `ExtRequest::method` and `ExtNotification::method`,
/// so a client call to `_vendor/method` travels as the subject token
/// `ext._vendor/method` and must not become `__vendor/method` on the way back.
pub(crate) fn ext_protocol_method(name: &str) -> Cow<'_, str> {
    if name.starts_with('_') {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(format!("_{name}"))
    }
}

impl std::fmt::Display for ExtMethodName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for ExtMethodName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests;
