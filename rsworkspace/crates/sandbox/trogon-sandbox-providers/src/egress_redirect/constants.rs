/// Environment variable carrying the NATS URL the egress proxy publishes violation events to.
pub const EGRESS_PROXY_VIOLATION_NATS_URL_ENV: &str = "EGRESS_PROXY_VIOLATION_NATS_URL";
/// Environment variable carrying the lease id stamped on violation events.
pub const EGRESS_PROXY_LEASE_ID_ENV: &str = "EGRESS_PROXY_LEASE_ID";
/// Environment variable telling the proxy which uid the redirect exempts.
pub const EGRESS_PROXY_EXEMPT_UID_ENV: &str = "EGRESS_PROXY_EXEMPT_UID";

/// Capabilities a workload must not hold while the proxy is exempted by uid: with either, it could become the
/// egress proxy uid.
pub const UID_CHANGE_CAPABILITIES: [&str; 2] = ["SETUID", "SETGID"];

/// The only port the egress proxy uid may send UDP to: it resolves names for the workload.
pub(super) const DNS_PORT: u16 = 53;

/// Present only when the namespace's kernel has an IPv6 stack; without one there is no IPv6 egress to filter.
pub(super) const IPV6_STACK_PROBE: &str = "/proc/net/if_inet6";
