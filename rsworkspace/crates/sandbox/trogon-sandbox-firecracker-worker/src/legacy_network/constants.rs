use std::time::Duration;

pub(super) const POLICY_FILE: &str = "policy.json";
pub(super) const PROXY_SOCKET: &str = "proxy.sock";
pub(super) const PROXY_LOG: &str = "egress-proxy.log";
pub(super) const PROXY_USER: &str = "sandbox";
pub(super) const NO_PROXY: &str = "localhost,127.0.0.1,::1";
pub(super) const READY_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const READY_POLL: Duration = Duration::from_millis(20);
pub(super) const TOKEN_BYTES: usize = 16;
pub(super) const MAX_NETNS_NAME: usize = 64;
/// What `fc-netns check` prints once it installs the anti-spoof guard.
pub const ANTI_SPOOF_REPLY: &str = "anti-spoof 1";
