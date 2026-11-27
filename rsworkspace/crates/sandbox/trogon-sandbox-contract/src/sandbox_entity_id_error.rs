/// An entity id is not a ULID, as [`SandboxEntityId`](crate::SandboxEntityId) requires.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SandboxEntityIdError {
    #[error("entity id must be {expected} characters")]
    Length { expected: usize },
    #[error("entity id must use Crockford base32 characters")]
    InvalidCharacter,
}
