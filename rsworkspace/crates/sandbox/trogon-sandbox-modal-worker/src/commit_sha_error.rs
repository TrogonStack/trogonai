/// A commit id that git could mistake for something else.
#[derive(Debug, thiserror::Error)]
pub enum CommitShaError {
    #[error("a commit id has {min} to {max} characters")]
    Length { min: usize, max: usize },
    #[error("a commit id holds hexadecimal digits only")]
    NotHex,
}
