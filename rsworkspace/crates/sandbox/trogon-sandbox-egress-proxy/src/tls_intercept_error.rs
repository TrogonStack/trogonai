use std::io;
use std::time::Duration;

use crate::http_intercept_error::HttpInterceptError;
use crate::oidc_signer::OidcSignerError;
use crate::tls_ca::SandboxCaError;

#[derive(Debug, thiserror::Error)]
pub enum TlsInterceptError {
    #[error("failed to mint leaf certificate: {0}")]
    LeafCertificate(SandboxCaError),
    #[error("TLS handshake with client failed: {0}")]
    ClientHandshake(io::Error),
    #[error("TLS handshake not done in time for {host}:{port}; tunnel closed")]
    ClientHandshakeTimedOut { host: String, port: u16, wait: Duration },
    #[error("failed to connect to upstream: {0}")]
    UpstreamConnect(io::Error),
    #[error("TLS handshake with upstream failed: {0}")]
    UpstreamHandshake(io::Error),
    #[error("invalid upstream server name {hostname}: {source}")]
    InvalidServerName {
        hostname: String,
        source: rustls::pki_types::InvalidDnsNameError,
    },
    #[error("splice between client and upstream failed: {0}")]
    Splice(io::Error),
    #[error("http intercept failed: {0}")]
    HttpIntercept(HttpInterceptError),
    #[error("invalid forward_url {forward_url}: {reason}")]
    InvalidForwardUrl { forward_url: String, reason: String },
    #[error("failed to connect to forward_url host {host}: {source}")]
    ForwardConnect { host: String, source: io::Error },
    #[error("TLS handshake with forward_url host {host} failed: {source}")]
    ForwardHandshake { host: String, source: io::Error },
    #[error("failed to mint OIDC token for forward_url: {0}")]
    OidcMint(OidcSignerError),
}

impl From<HttpInterceptError> for TlsInterceptError {
    fn from(error: HttpInterceptError) -> Self {
        match error {
            HttpInterceptError::UpstreamConnect(source) => Self::UpstreamConnect(source),
            HttpInterceptError::UpstreamHandshake(source) => Self::UpstreamHandshake(source),
            other => Self::HttpIntercept(other),
        }
    }
}

#[cfg(test)]
mod tests;
