/// A string is not an IP range in CIDR notation.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("'{0}' is not an IP range in CIDR notation such as 10.0.0.0/8")]
pub struct IpRangeError(pub(crate) String);

impl IpRangeError {
    /// The rejected input as written.
    pub fn input(&self) -> &str {
        &self.0
    }
}
