use std::io;
use std::path::PathBuf;

/// The host side of a lease's relay: the unix socket its proxy serves and the
/// binary that runs as the relay inside the sandbox.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RelayEgress {
    pub proxy_socket: PathBuf,
    pub relay_binary: PathBuf,
}

impl RelayEgress {
    pub fn with_current_binary(proxy_socket: PathBuf) -> io::Result<Self> {
        Ok(Self {
            proxy_socket,
            relay_binary: std::env::current_exe()?,
        })
    }
}
