/// Describes what went wrong when validating a NATS subject token: empty, invalid character, or too long.
#[derive(Debug, Clone, PartialEq)]
pub enum SubjectTokenViolation {
    Empty,
    InvalidCharacter(char),
    TooLong(usize),
}
