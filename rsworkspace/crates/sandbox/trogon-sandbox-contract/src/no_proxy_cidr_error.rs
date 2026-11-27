/// A string is not a [`NoProxyCidr`](crate::NoProxyCidr).
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum NoProxyCidrError {
    #[error("'{0}' is not an IP address with an optional prefix length")]
    Invalid(String),
    #[error("the prefix length of '{input}' is above {max}")]
    PrefixTooLong { input: String, max: u8 },
}
