/// A string is not a claim name [`ClaimName`](crate::ClaimName) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ClaimNameError {
    #[error("claim name must not be empty")]
    Empty,
}
