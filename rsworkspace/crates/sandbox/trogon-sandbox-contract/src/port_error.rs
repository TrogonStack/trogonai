/// A number or string is not a port from 1 to 65535 without leading zeros.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("port '{0}' is not in 1-65535 without leading zeros")]
pub struct PortError(pub(crate) String);

impl PortError {
    /// The rejected input as written.
    pub fn input(&self) -> &str {
        &self.0
    }
}
