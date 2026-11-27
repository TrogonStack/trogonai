use std::time::Duration;
use trogon_sandbox_contract::SocketMark;

pub const UPSTREAM_MARK: SocketMark = SocketMark::EGRESS_PROXY;

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) const MAX_CONNECT_RESPONSE_HEAD: usize = 16 * 1024;
