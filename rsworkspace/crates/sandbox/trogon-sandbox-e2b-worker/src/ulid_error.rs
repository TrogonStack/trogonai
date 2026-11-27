/// Why a string is not a ULID.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum UlidError {
    #[error("a ULID is 26 characters")]
    Length,
    #[error("a ULID uses only Crockford base32 characters")]
    InvalidCharacter,
    #[error("a ULID must not exceed 128 bits")]
    Overflow,
}
