use std::net::SocketAddr;

use trogon_sandbox_ssh_gateway::KeyError;

use crate::GuestAccessError;

/// Why the worker has no SSH session into a guest. Terminals see it as
/// [`trogon_sandbox_ssh_gateway::GuestError::Unavailable`].
#[derive(Debug, thiserror::Error)]
pub enum GuestSessionError {
    /// The worker already keeps a session for as many leases as it may.
    #[error("worker is at its limit of {max} guest sessions")]
    AtLimit {
        /// The limit.
        max: usize,
    },
    /// The ephemeral session key could not be generated.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The guest could not be located or its key installed.
    #[error(transparent)]
    Access(#[from] GuestAccessError),
    /// The guest's `sshd` did not finish connecting in time.
    #[error("timed out connecting to the guest's sshd at {addr}")]
    ConnectTimeout {
        /// Where the guest's `sshd` listens.
        addr: SocketAddr,
    },
    /// The connection to the guest's `sshd` failed.
    #[error("ssh connect to {addr} failed: {source}")]
    Connect {
        /// Where the guest's `sshd` listens.
        addr: SocketAddr,
        /// Why it failed.
        #[source]
        source: russh::Error,
    },
    /// Authentication could not be attempted.
    #[error("ssh authentication failed: {0}")]
    Authenticate(#[source] russh::Error),
    /// The guest's `sshd` did not accept the ephemeral key.
    #[error("the guest's sshd refused the ephemeral key for {user}")]
    Refused {
        /// The guest account.
        user: &'static str,
    },
    /// The session could not open a channel.
    #[error("could not open an ssh channel: {0}")]
    Channel(#[source] russh::Error),
}
