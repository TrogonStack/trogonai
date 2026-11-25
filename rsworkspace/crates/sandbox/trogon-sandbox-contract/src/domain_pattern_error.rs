use crate::{HostError, PortError};

/// A string is not a destination pattern [`DomainPattern`](crate::DomainPattern) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DomainPatternError {
    #[error("domain pattern must not be empty")]
    Empty,
    #[error("IPv6 literal or extra ':' in '{0}': IPv6 must be bracketed")]
    UnbracketedIpv6(String),
    #[error("malformed bracketed address in '{0}'")]
    MalformedBrackets(String),
    #[error(transparent)]
    Port(#[from] PortError),
    #[error("wildcard in '{0}' must be a single leading '*.' with at least two labels after it")]
    InvalidWildcard(String),
    #[error("wildcard in '{input}' must be a single leading '*.' with at least two labels after it")]
    InvalidWildcardHost {
        input: String,
        #[source]
        source: HostError,
    },
    #[error("bare '*' is only allowed in denied domains")]
    BareWildcardNotAllowed,
    #[error("'{0}' must contain a dot or be 'localhost'")]
    SingleLabel(String),
    #[error(transparent)]
    Host(#[from] HostError),
}
