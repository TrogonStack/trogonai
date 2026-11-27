use trogon_sandbox_contract::SandboxIdError;

/// Why a SPACE identifier was refused.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SpaceIdError {
    /// The identifier is not a usable sandbox identifier at all, such as an empty one.
    #[error(transparent)]
    Sandbox(#[from] SandboxIdError),
    /// The identifier is longer than allowed.
    #[error("SPACE ID is longer than {max} bytes")]
    TooLong {
        /// The longest identifier accepted.
        max: usize,
    },
    /// The identifier holds a character that could alter a request path.
    #[error("SPACE ID holds a character outside letters, digits, `_` and `-`")]
    Character,
}
