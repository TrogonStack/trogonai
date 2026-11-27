/// Why an authority resource identity was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum AuthorityResourceIdError {
    /// The identity is empty.
    #[error("authority resource identity must not be empty")]
    Empty,
    /// The identity is longer than the bound.
    #[error("authority resource identity must be at most {max} bytes")]
    TooLong {
        /// The longest accepted identity in bytes.
        max: usize,
    },
    /// The identity contains a NUL byte.
    #[error("authority resource identity must not contain NUL")]
    NulByte,
}
