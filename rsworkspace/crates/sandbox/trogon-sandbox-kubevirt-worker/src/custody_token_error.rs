/// A custody token was not a UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a custody token is a UUID")]
pub struct CustodyTokenError;
