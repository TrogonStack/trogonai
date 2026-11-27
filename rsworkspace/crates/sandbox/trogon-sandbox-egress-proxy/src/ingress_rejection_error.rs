use std::io;

/// Why a connection never reached policy evaluation. None of these are
/// policy denials, so none produce a violation event.
#[derive(Debug, thiserror::Error)]
pub enum IngressRejectionError {
    #[error("first byte is not HTTP, SOCKS4 or SOCKS5")]
    UnknownProtocol,
    #[error("missing or wrong proxy token")]
    Unauthenticated,
    #[error("malformed request: {0}")]
    Malformed(&'static str),
    #[error("malformed host: {host:?}:{port}")]
    MalformedHost { host: String, port: u16 },
    #[error("unsupported SOCKS5 command")]
    UnsupportedCommand,
    #[error("SOCKS4 is not supported")]
    UnsupportedSocksVersion,
    #[error("request head too large")]
    HeadTooLarge,
    #[error("handshake timed out")]
    HandshakeTimedOut,
    #[error("concurrent tunnel limit reached")]
    TunnelLimitReached,
    #[error("i/o error: {0}")]
    Io(io::Error),
}

impl From<io::Error> for IngressRejectionError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
