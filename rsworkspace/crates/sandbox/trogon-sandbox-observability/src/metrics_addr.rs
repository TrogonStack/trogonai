use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::constants::METRICS_PORT_OFFSET;

/// Default Prometheus scrape address: loopback on `service_port + 1000`.
///
/// The port saturates at `u16::MAX`.
pub fn metrics_addr(service_port: u16) -> SocketAddr {
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        service_port.saturating_add(METRICS_PORT_OFFSET),
    )
}
