use crate::GatewayAccessError;

/// Why the worker has no SSH session into Daytona's gateway. Terminals see it as
/// [`trogon_sandbox_ssh_gateway::GuestError::Unavailable`]. No variant carries the access token.
#[derive(Debug, thiserror::Error)]
pub enum GatewaySessionError {
    /// The worker already keeps a session for as many leases as it may.
    #[error("worker is at its limit of {max} gateway sessions")]
    AtLimit { max: usize },
    /// No access token could be had.
    #[error(transparent)]
    Access(#[from] GatewayAccessError),
    /// The gateway did not finish connecting in time.
    #[error("timed out connecting to Daytona's SSH gateway at {host}:{port}")]
    ConnectTimeout { host: String, port: u16 },
    /// The connection to the gateway failed.
    #[error("ssh connect to {host}:{port} failed: {source}")]
    Connect {
        host: String,
        port: u16,
        #[source]
        source: russh::Error,
    },
    /// Authentication could not be attempted.
    #[error("ssh authentication failed: {0}")]
    Authenticate(#[source] russh::Error),
    /// The gateway refused a token it had just minted.
    #[error("Daytona's SSH gateway refused a freshly minted access token")]
    RefusedFreshToken,
    /// The session could not open a channel.
    #[error("could not open an ssh channel: {0}")]
    Channel(#[source] russh::Error),
}
