use std::fmt;
use std::str::FromStr;

const MAX_LENGTH: usize = 256;

/// An opaque principal identifier carried by provisioning actor and owner fields.
/// The domain never distinguishes human from machine principals: that
/// distinction is authentication context (aauth, ADR-0017), an app-level
/// concern enforced at command dispatch, not inside `decide`. `Principal`
/// carries no kind tag and compares by value only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Principal(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalViolation {
    #[error("must not be empty")]
    Empty,
    #[error("must not have leading or trailing whitespace")]
    SurroundingWhitespace,
    #[error("must not exceed {max} characters, got {actual}")]
    TooLong { max: usize, actual: usize },
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("principal '{raw}' is invalid: {violation}")]
pub struct PrincipalError {
    raw: String,
    violation: PrincipalViolation,
}

impl Principal {
    pub fn parse(raw: &str) -> Result<Self, PrincipalError> {
        if raw.is_empty() {
            return Err(PrincipalError::new(raw, PrincipalViolation::Empty));
        }

        if raw.trim() != raw {
            return Err(PrincipalError::new(raw, PrincipalViolation::SurroundingWhitespace));
        }

        if raw.len() > MAX_LENGTH {
            return Err(PrincipalError::new(
                raw,
                PrincipalViolation::TooLong {
                    max: MAX_LENGTH,
                    actual: raw.len(),
                },
            ));
        }

        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Principal {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for Principal {
    type Err = PrincipalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PrincipalError {
    fn new(raw: &str, violation: PrincipalViolation) -> Self {
        Self {
            raw: raw.to_string(),
            violation,
        }
    }
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
