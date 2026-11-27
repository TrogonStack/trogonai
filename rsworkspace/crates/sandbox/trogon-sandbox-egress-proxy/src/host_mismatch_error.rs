/// Why the `Host` header, an absolute-form authority, or the TLS server name
/// does not name the connection's TLS target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostMismatchError {
    #[error("request-target authority does not match the request target")]
    Authority,
    #[error("more than one Host header")]
    MultipleHeaders,
    #[error("Host header does not match the request target")]
    Header,
    #[error("TLS server name does not match the request target")]
    ServerName,
    #[error("TLS server name could not be read")]
    ServerNameUnreadable,
}
