/// A snapshot generation is not a canonical ULID, as [`SnapshotGeneration`](crate::SnapshotGeneration)
/// requires.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotGenerationError {
    #[error("snapshot generation must be {expected} characters")]
    Length { expected: usize },
    #[error("snapshot generation must use uppercase Crockford base32 characters")]
    InvalidCharacter,
    #[error("snapshot generation must start with 0 through 7 to fit in 128 bits")]
    Overflow,
}
