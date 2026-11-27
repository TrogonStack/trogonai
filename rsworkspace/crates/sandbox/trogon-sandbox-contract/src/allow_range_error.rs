/// A string is not a CIDR range, an IP address, or an IP address with a port.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("'{0}' is not a CIDR range, an IP address, or an IP address with a port such as 10.0.0.5:8080")]
pub struct AllowRangeError(pub(crate) String);

impl AllowRangeError {
    /// The rejected input as written.
    pub fn input(&self) -> &str {
        &self.0
    }
}
