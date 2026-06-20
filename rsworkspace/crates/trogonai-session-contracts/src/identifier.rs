//! Shared validation for durable contract identifier strings.

const MAX_IDENTIFIER_LEN: usize = 128;

/// Error returned when an identifier fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierError {
    Empty,
    TooLong(usize),
    InvalidCharacter(char),
}

impl std::fmt::Display for IdentifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "identifier must not be empty"),
            Self::TooLong(len) => {
                write!(
                    f,
                    "identifier is too long: {len} characters (max {MAX_IDENTIFIER_LEN})"
                )
            }
            Self::InvalidCharacter(ch) => {
                write!(f, "identifier contains invalid character: {ch:?}")
            }
        }
    }
}

impl std::error::Error for IdentifierError {}

pub(crate) fn validate_identifier(value: &str) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty);
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        return Err(IdentifierError::TooLong(value.len()));
    }
    if value.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        let invalid = value
            .chars()
            .find(|ch| ch.is_control() || ch.is_whitespace())
            .expect("invalid character exists");
        return Err(IdentifierError::InvalidCharacter(invalid));
    }
    Ok(())
}

pub(crate) fn validate_prefixed_identifier(
    value: &str,
    expected_prefix: &str,
) -> Result<(), IdentifierError> {
    validate_identifier(value)?;
    if !value.starts_with(expected_prefix) {
        return Err(IdentifierError::InvalidCharacter('_'));
    }
    Ok(())
}
