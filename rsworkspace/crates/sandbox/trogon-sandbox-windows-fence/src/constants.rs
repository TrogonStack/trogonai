use crate::proxy_port_range::ProxyPortRange;

/// What opening loopback does on Windows, in plain words.
pub const WINDOWS_ALLOW_LOOPBACK_EFFECT: &str = "Loopback is open: sandboxed commands can connect to every TCP and UDP port on localhost (127.0.0.0/8 and ::1; port 1 alone stays blocked, so the sandbox can check its own fence). Every service listening on localhost is reachable from them, and any local forwarder (a DNS stub, a SOCKS or HTTP proxy, a VPN or security agent, a tunnel) reachable there gives them a way off the machine that the network restrictions do not cover.";

/// The one loopback port that stays blocked for the sandbox account with
/// loopback open, so the fence check has a connect that must be refused.
pub const LOOPBACK_PROBE_PORT: u16 = 1;

pub const DEFAULT_SANDBOX_USER: &str = "trogon-sandbox";

pub const DEFAULT_PROXY_PORT_RANGE: ProxyPortRange = ProxyPortRange::reported(60080, 60089);

/// `no_proxy` for a sandboxed child with loopback open: the
/// loopback hosts, so proxy-aware clients connect to local servers directly.
/// Otherwise `no_proxy` is not set at all.
pub const WINDOWS_LOOPBACK_NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// How long to wait for the self-elevating install: the Windows UAC consent
/// dialog dismisses itself after about two minutes, and elevation is not
/// retracted when the parent dies, so anything shorter risks half-completing
/// a late approval.
pub const INSTALL_TIMEOUT_MS: u64 = 120_000;

/// How long the profile read grant may take: the first run touches every
/// file under the granted directories.
pub const PROFILE_GRANT_TIMEOUT_MS: u64 = 30 * 60_000;

/// The first argument that routes a multicall executable to `fence-helper`.
pub const DISPATCH_ARG1: &str = "--fence-helper";

/// Behaviours of `fence-helper` this library relies on and asks for by name.
/// `fence-helper` lists what it does as `capabilities` in `user status`.
pub const REQUIRED_CAPABILITIES: [&str; 5] = [
    "profile-deny-expansion",
    "kept-grant-holders",
    "profile-session-grant-check",
    "read-deny-except",
    "exec-holder",
];

/// What the capabilities are needed for, as the outdated error words it.
pub const CAPABILITIES_WHY: &str = "a session depends on (its read denies are written on the profile read grant's paths by fence-helper, and kept grants are checked and removed there)";
