use std::fmt;

#[derive(thiserror::Error)]
pub enum RuntimeError {
    #[error("peer connection startup deadline elapsed")]
    StartupDeadline,
    #[error("peer connection failed")]
    Connect(#[source] trogon_nats::ConnectError),
    #[error("peer MCP initialization failed")]
    Initialize(#[source] Box<rmcp::service::ServerInitializeError>),
    #[error("peer connection readiness failed")]
    ConnectReady(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("peer connector configuration failed")]
    Configure(#[source] crate::peer_mcp_error::PeerMcpError),
    #[error("peer connector shutdown failed")]
    Connector(#[source] crate::peer_mcp_error::PeerMcpError),
    #[error("peer MCP shutdown failed")]
    Shutdown(#[source] tokio::task::JoinError),
    #[error("peer MCP shutdown deadline elapsed")]
    ShutdownDeadline,
}
impl fmt::Debug for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
