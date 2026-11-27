use serde::Deserialize;

/// One published container port, as `Sandbox.tunnels()` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModalTunnel {
    /// Container port the tunnel forwards to.
    pub port: u16,
    pub host: String,
    /// Public TLS port on `host`.
    pub tls_port: u16,
    /// `https://` URL Modal gives for the tunnel.
    pub url: String,
}
