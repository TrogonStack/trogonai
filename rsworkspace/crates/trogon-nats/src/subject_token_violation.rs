/// Describes what went wrong when validating a NATS subject token: empty, invalid character, or too long.
#[derive(Debug, Clone, PartialEq)]
pub enum SubjectTokenViolation {
    Empty,
    InvalidCharacter(char),
    TooLong(usize),
}

impl std::fmt::Display for SubjectTokenViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("subject token is empty"),
            Self::InvalidCharacter(ch) => write!(f, "subject token contains invalid character '{ch}'"),
            Self::TooLong(length) => write!(f, "subject token exceeds maximum length: {length}"),
        }
    }
}

impl std::error::Error for SubjectTokenViolation {}
