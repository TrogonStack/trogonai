/// A sandbox identifier does not satisfy the rules [`SandboxId`](crate::SandboxId) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SandboxIdError {
    #[error("sandbox id must not be empty")]
    Empty,
    #[error("sandbox id must be at most {max} bytes")]
    TooLong { max: usize },
    #[error("sandbox id must not contain control characters")]
    ControlCharacter,
}
