/// Why a SPACE token was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SpaceTokenError {
    /// The token is empty.
    #[error("SPACE token is empty")]
    Empty,
    /// The token holds characters a header cannot carry.
    #[error("SPACE token is not a valid header value")]
    NotAHeaderValue,
}
