use std::time::Duration;

pub const BRIDGE_FLAG: &str = "--port-bridge";

pub(super) const CONNECTED: u8 = 0;

pub(super) const REFUSED: u8 = 1;

pub(super) const FAILED: u8 = 2;

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) const MAX_DISCOVERY_PROCESSES: usize = 256;

pub(super) const MAX_DISCOVERY_DEPTH: usize = 64;
