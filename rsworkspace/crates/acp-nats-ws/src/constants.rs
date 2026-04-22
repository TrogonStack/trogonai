use std::net::{IpAddr, Ipv4Addr};

pub const ACP_CONNECTION_ID_HEADER: &str = "acp-connection-id";
pub const ACP_ENDPOINT: &str = "/acp";
pub const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
pub const DEFAULT_PORT: u16 = 8080;
pub const DUPLEX_BUFFER_SIZE: usize = 64 * 1024;
pub const LEGACY_WS_ENDPOINT: &str = "/ws";
pub const THREAD_NAME: &str = "acp-ws-local";
