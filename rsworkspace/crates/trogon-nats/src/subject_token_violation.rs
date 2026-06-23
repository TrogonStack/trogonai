/// Describes what went wrong when validating a NATS subject token: empty, invalid character, or too long.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SubjectTokenViolation {
    #[error("subject token is empty")]
    Empty,
    #[error("subject token contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("subject token exceeds maximum length: {0}")]
    TooLong(usize),
}

#[cfg(test)]
mod tests;
