//! NATS-safe extension method name value object.
//!
//! Method names are embedded in subjects: `{prefix}.agent.ext.{method}`.
//! Validation follows [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names):
//! rejects `*`, `>`, whitespace; allows dotted namespaces (e.g. `vendor.operation`) but rejects
//! malformed dots (consecutive, leading, trailing). Validity is guaranteed at construction.

use std::sync::Arc;

use crate::nats::token;
use crate::subject_token_violation::SubjectTokenViolation;

const MAX_METHOD_NAME_LENGTH: usize = 128;

/// Error returned when [`ExtMethodName`] validation fails.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtMethodNameError(pub SubjectTokenViolation);

impl std::fmt::Display for ExtMethodNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            SubjectTokenViolation::Empty => write!(f, "method must not be empty"),
            SubjectTokenViolation::InvalidCharacter(ch) => {
                write!(f, "method contains invalid character: {:?}", ch)
            }
            SubjectTokenViolation::TooLong(len) => {
                write!(f, "method is too long: {} bytes (max 128)", len)
            }
        }
    }
}

impl std::error::Error for ExtMethodNameError {}

/// NATS-safe extension method name. Guarantees validity at construction—invalid instances are unrepresentable.
///
/// Rejects empty, too-long, wildcard, whitespace, and malformed dotted names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExtMethodName(Arc<str>);

impl ExtMethodName {
    pub fn new(method: impl AsRef<str>) -> Result<Self, ExtMethodNameError> {
        let s = method.as_ref();
        if s.is_empty() {
            return Err(ExtMethodNameError(SubjectTokenViolation::Empty));
        }
        if s.len() > MAX_METHOD_NAME_LENGTH {
            return Err(ExtMethodNameError(SubjectTokenViolation::TooLong(s.len())));
        }
        if let Some(ch) = token::has_wildcards_or_whitespace(s) {
            return Err(ExtMethodNameError(SubjectTokenViolation::InvalidCharacter(
                ch,
            )));
        }
        if token::has_consecutive_or_boundary_dots(s) {
            return Err(ExtMethodNameError(SubjectTokenViolation::InvalidCharacter(
                '.',
            )));
        }
        Ok(Self(s.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
mod tests {
    use super::*;

    #[test]
    fn ext_method_name_valid() {
        assert!(ExtMethodName::new("my_custom_method").is_ok());
        assert!(ExtMethodName::new("simple").is_ok());
        assert!(ExtMethodName::new("method123").is_ok());
    }

    #[test]
    fn ext_method_name_too_long_returns_err() {
        let long = "a".repeat(129);
        let err = ExtMethodName::new(&long).err().unwrap();
        assert_eq!(err, ExtMethodNameError(SubjectTokenViolation::TooLong(129)));
    }

    #[test]
    fn ext_method_name_dotted_namespaces_accepted() {
        assert!(ExtMethodName::new("my.custom.method").is_ok());
        assert!(ExtMethodName::new("a.b").is_ok());
        assert!(ExtMethodName::new("vendor.operation").is_ok());
    }

    #[test]
    fn ext_method_name_malformed_dots_rejected() {
        assert!(ExtMethodName::new("..method").is_err());
        assert!(ExtMethodName::new("method..name").is_err());
        assert!(ExtMethodName::new(".method").is_err());
        assert!(ExtMethodName::new("method.").is_err());
        assert!(ExtMethodName::new(".").is_err());
    }

    #[test]
    fn ext_method_name_empty_returns_err() {
        let err = ExtMethodName::new("").err().unwrap();
        assert_eq!(err, ExtMethodNameError(SubjectTokenViolation::Empty));
    }

    #[test]
    fn ext_method_name_wildcard_returns_err() {
        assert!(ExtMethodName::new("method.*").is_err());
        assert!(ExtMethodName::new("method.>").is_err());
    }

    #[test]
    fn ext_method_name_whitespace_returns_err() {
        assert!(ExtMethodName::new("method name").is_err());
        assert!(ExtMethodName::new("method\t").is_err());
    }

    #[test]
    fn ext_method_name_display_and_deref() {
        let name = ExtMethodName::new("my_method").unwrap();
        assert_eq!(format!("{}", name), "my_method");
        assert_eq!(name.len(), 9);
        assert!(name.starts_with("my"));
    }

    #[test]
    fn ext_method_name_error_display() {
        assert_eq!(
            format!("{}", ExtMethodNameError(SubjectTokenViolation::Empty)),
            "method must not be empty"
        );
        assert_eq!(
            format!(
                "{}",
                ExtMethodNameError(SubjectTokenViolation::InvalidCharacter(' '))
            ),
            "method contains invalid character: ' '"
        );
        assert_eq!(
            format!(
                "{}",
                ExtMethodNameError(SubjectTokenViolation::TooLong(200))
            ),
            "method is too long: 200 bytes (max 128)"
        );
    }
}
