use trogon_sandbox_egress_proxy::ingress::InvalidProxyTokenError;
use trogon_sandbox_egress_proxy::tls_ca::SandboxCaError;

/// A lease's egress proxy could not be started or stopped.
#[derive(Debug, thiserror::Error)]
pub enum LeaseProxyError {
    #[error("the proxy certificate authority could not be built: {0}")]
    Authority(#[from] SandboxCaError),
    #[error("the proxy could not listen or stop: {0}")]
    Io(#[from] std::io::Error),
    #[error("the proxy token is unusable: {0}")]
    Token(#[from] InvalidProxyTokenError),
    #[error("the proxy listener is not a TCP listener")]
    NotTcp,
}
