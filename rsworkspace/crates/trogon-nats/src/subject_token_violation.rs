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
mod tests {
    use super::*;

    #[test]
    fn display_formats_each_variant() {
        assert_eq!(SubjectTokenViolation::Empty.to_string(), "subject token is empty");
        assert_eq!(
            SubjectTokenViolation::InvalidCharacter('*').to_string(),
            "subject token contains invalid character '*'"
        );
        assert_eq!(
            SubjectTokenViolation::TooLong(129).to_string(),
            "subject token exceeds maximum length: 129"
        );
    }

    #[test]
    fn violation_implements_error() {
        let error: &dyn std::error::Error = &SubjectTokenViolation::Empty;

        assert_eq!(error.to_string(), "subject token is empty");
    }
}
