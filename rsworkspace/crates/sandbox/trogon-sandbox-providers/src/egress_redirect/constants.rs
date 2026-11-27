/// Environment variable carrying the NATS URL the egress proxy publishes violation events to.
pub const EGRESS_PROXY_VIOLATION_NATS_URL_ENV: &str = "EGRESS_PROXY_VIOLATION_NATS_URL";
/// Environment variable carrying the lease id stamped on violation events.
pub const EGRESS_PROXY_LEASE_ID_ENV: &str = "EGRESS_PROXY_LEASE_ID";
/// Environment variable telling the proxy which uid the redirect exempts.
pub const EGRESS_PROXY_EXEMPT_UID_ENV: &str = "EGRESS_PROXY_EXEMPT_UID";

/// Capabilities a workload must not hold while the proxy is exempted by uid: with either, it could become the
/// egress proxy uid.
pub const UID_CHANGE_CAPABILITIES: [&str; 2] = ["SETUID", "SETGID"];
