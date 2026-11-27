//! The fixed addresses and names of a lease's guest-facing link, shared by the VMM API and
//! the lease networks.

use std::net::{SocketAddr, SocketAddrV4};

mod constants;
pub use constants::{GUEST_ADDR, GUEST_INTERFACE, GUEST_MAC, HOST_ADDR, PROXY_PORT, TAP_DEVICE};

pub fn relay_listen_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(HOST_ADDR, PROXY_PORT))
}

/// Configures the guest's interface from the kernel command line, so the rootfs needs no
/// network configuration of its own and has no DHCP server to wait for.
pub fn guest_ip_boot_arg() -> String {
    format!("ip={GUEST_ADDR}::{HOST_ADDR}:255.255.255.252::{GUEST_INTERFACE}:off")
}
