/// A list of redact values does not satisfy the rules [`RedactValues`](crate::RedactValues) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RedactValuesError {
    #[error("at most {max} values can be registered for redaction")]
    TooMany { max: usize },
    #[error("a value registered for redaction exceeds {max} bytes")]
    TooLong { max: usize },
}
