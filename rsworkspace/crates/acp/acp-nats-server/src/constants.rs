//! Crate-wide constants.

use std::net::{IpAddr, Ipv4Addr};

pub const ACP_CONNECTION_ID_HEADER: &str = "acp-connection-id";
/// Only the tests name the endpoint now: it matches `ServerOptions::default`,
/// so upstream owns the served path rather than this crate.
#[cfg(test)]
pub const ACP_ENDPOINT: &str = "/acp";
pub const ACP_PROTOCOL_VERSION_HEADER: &str = "acp-protocol-version";
pub const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
pub const DEFAULT_PORT: u16 = 8080;
pub const X_ACCEL_BUFFERING_HEADER: &str = "x-accel-buffering";

/// Largest `initialize` response body inspected for the negotiated version.
///
/// Only the JSON `initialize` reply is ever buffered, and that payload is
/// capability metadata, so this is generous. Anything larger passes through
/// unread rather than being rejected: failing a valid response to enforce a
/// SHOULD-level header check would be the worse trade.
pub(crate) const MAX_INSPECTED_BODY: usize = 1024 * 1024;

/// Connections retained before the oldest is evicted.
///
/// `DELETE` is the only close this layer can observe: upstream keeps connection
/// lifetime private, so a connection that ends by peer drop, client crash, or
/// process drain never reports it. Without a cap, every such connection would
/// leave an entry behind and repeated `initialize` calls would grow the map for
/// the life of the process.
///
/// Evicting is safe because a missing entry means validation is skipped for that
/// connection, which is already how an id this layer never saw initialize is
/// treated. The cost of eviction is a missed check, not a rejected request.
pub(crate) const MAX_TRACKED_CONNECTIONS: usize = 4096;
