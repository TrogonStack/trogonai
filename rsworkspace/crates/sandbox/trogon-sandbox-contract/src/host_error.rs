/// A string is not a host name or IP address [`Host`](crate::Host) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum HostError {
    #[error("host must not be empty")]
    Empty,
    #[error("host '{0}' is not a valid host name or IP address")]
    Invalid(String),
}
