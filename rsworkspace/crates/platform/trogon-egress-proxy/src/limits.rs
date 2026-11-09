use std::time::Duration;

use crate::connection_limit::ConnectionLimit;

/// Bounds that keep one slow or hostile client from holding the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Incomplete handshakes and established tunnels share this task budget.
    pub max_connections: ConnectionLimit,
    /// Largest accepted request head, request line and headers together.
    pub max_head_bytes: usize,
    pub handshake_timeout: Duration,
    pub head_timeout: Duration,
    pub connect_timeout: Duration,
    /// How long a client has to send its first TLS record once the tunnel is
    /// open.
    pub first_record_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: ConnectionLimit::default(),
            max_head_bytes: 8 * 1024,
            handshake_timeout: Duration::from_secs(10),
            head_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            first_record_timeout: Duration::from_secs(10),
        }
    }
}
