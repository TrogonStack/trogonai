use std::net::Ipv4Addr;

pub const HOST_ADDR: Ipv4Addr = Ipv4Addr::new(172, 16, 0, 1);
pub const GUEST_ADDR: Ipv4Addr = Ipv4Addr::new(172, 16, 0, 2);
pub const PROXY_PORT: u16 = 3128;
pub const TAP_DEVICE: &str = "tap0";
pub const GUEST_INTERFACE: &str = "eth0";
pub const GUEST_MAC: &str = "06:00:ac:10:00:02";
