/// A minimum protocol version override that is set but cannot be read as an unsigned integer.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("environment variable {name} must be an unsigned integer")]
pub struct MinProtocolVersionError {
    pub name: String,
}
