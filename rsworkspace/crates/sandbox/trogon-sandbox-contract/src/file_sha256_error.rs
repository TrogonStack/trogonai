/// A checksum does not satisfy the lowercase SHA-256 hexadecimal grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum FileSha256Error {
    #[error("file checksum must be exactly {expected} characters")]
    Length { expected: usize },
    #[error("file checksum must be lowercase hexadecimal")]
    NotLowercaseHex,
}
