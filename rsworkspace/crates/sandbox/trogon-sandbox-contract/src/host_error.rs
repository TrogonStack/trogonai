use std::net::AddrParseError;

/// A string is not a host name or IP address [`Host`](crate::Host) accepts.
///
/// Every rejection but an empty input reads the same, and the variants keep the parser's own error as the source
/// when there is one.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum HostError {
    #[error("host must not be empty")]
    Empty,
    #[error("host '{0}' is not a valid host name or IP address")]
    Invalid(String),
    #[error("host '{input}' is not a valid host name or IP address")]
    InvalidIpv6 {
        input: String,
        #[source]
        source: AddrParseError,
    },
    #[error("host '{input}' is not a valid host name or IP address")]
    Unparsable {
        input: String,
        #[source]
        source: url::ParseError,
    },
}
