use crate::credential_source::CredentialSourceError;
use crate::invalid_match_rule_error::InvalidMatchRuleError;
use crate::refusal_error::RefusalError;
use crate::stream_use::StreamUse;

#[derive(Debug, thiserror::Error)]
pub enum HttpInterceptError {
    #[error("failed to read from client: {0}")]
    ClientRead(std::io::Error),
    #[error("failed to connect to upstream: {0}")]
    UpstreamConnect(std::io::Error),
    #[error("TLS handshake with upstream failed: {0}")]
    UpstreamHandshake(std::io::Error),
    #[error("failed to write to upstream: {0}")]
    UpstreamWrite(std::io::Error),
    #[error("failed to read from upstream: {0}")]
    UpstreamRead(std::io::Error),
    #[error("failed to write to client: {0}")]
    ClientWrite(std::io::Error),
    #[error("malformed HTTP message: {0}")]
    MalformedRequest(String),
    #[error("request refused: {0}")]
    Refused(RefusalError),
    #[error("credential broker error: {0}")]
    Credential(#[from] CredentialSourceError),
    #[error(transparent)]
    InvalidMatchRule(#[from] InvalidMatchRuleError),
}

impl HttpInterceptError {
    /// Whether the upstream connection was closed or reset under the request,
    /// which the workload's HTTP stack reports as `ECONNRESET` or `EPIPE`.
    fn upstream_gone(&self) -> bool {
        match self {
            Self::UpstreamRead(error) | Self::UpstreamWrite(error) => matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ),
            _ => false,
        }
    }

    /// Whether the client's connection is closed with no answer: the upstream
    /// closed a kept-alive connection as the request went out, which the proxy
    /// passes on as the client would see it on a direct connection.
    pub(crate) fn ends_silently(&self, stream_use: StreamUse) -> bool {
        stream_use == StreamUse::Reused && self.upstream_gone()
    }
}

#[cfg(test)]
mod tests;
