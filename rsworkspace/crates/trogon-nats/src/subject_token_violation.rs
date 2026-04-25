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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_are_equal_to_themselves() {
        assert_eq!(SubjectTokenViolation::Empty, SubjectTokenViolation::Empty);
        assert_eq!(
            SubjectTokenViolation::InvalidCharacter('.'),
            SubjectTokenViolation::InvalidCharacter('.')
        );
        assert_eq!(
            SubjectTokenViolation::TooLong(200),
            SubjectTokenViolation::TooLong(200)
        );
    }

    #[test]
    fn variants_are_not_equal_to_each_other() {
        assert_ne!(
            SubjectTokenViolation::Empty,
            SubjectTokenViolation::TooLong(1)
        );
        assert_ne!(
            SubjectTokenViolation::InvalidCharacter('*'),
            SubjectTokenViolation::InvalidCharacter('>')
        );
        assert_ne!(
            SubjectTokenViolation::TooLong(10),
            SubjectTokenViolation::TooLong(20)
        );
    }

    #[test]
    fn clone_produces_equal_value() {
        let v = SubjectTokenViolation::InvalidCharacter('x');
        assert_eq!(v.clone(), v);
    }

    #[test]
    fn debug_format_is_non_empty() {
        assert!(!format!("{:?}", SubjectTokenViolation::Empty).is_empty());
        assert!(!format!("{:?}", SubjectTokenViolation::InvalidCharacter('.')).is_empty());
        assert!(!format!("{:?}", SubjectTokenViolation::TooLong(128)).is_empty());
    }

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
