/// A terminal id does not satisfy the wire grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TerminalIdError {
    #[error("terminal id must be 1 to {max} characters")]
    InvalidLength { max: usize },
    #[error("terminal id must contain only ASCII letters, digits, '-' or '_'")]
    InvalidCharacters,
}
