/// Seconds an observed name-to-address binding stays live.
pub const BINDING_TTL_SECS: &str = "EGRESS_PROXY_BINDING_TTL_SECS";

/// Seconds between revalidations of open flows; unset or `0` revalidates only
/// when the policy changes. Implies live-flow tracking.
pub const REVALIDATE_INTERVAL_SECS: &str = "EGRESS_PROXY_REVALIDATE_INTERVAL_SECS";

/// `true` cancels live flows a policy replacement no longer admits.
pub const RESWEEP_LIVE_FLOWS: &str = "EGRESS_PROXY_RESWEEP_LIVE_FLOWS";

/// `true` terminates TLS for allows that rest on the presented name alone.
pub const REQUIRE_TERMINATION_FOR_NAME_ONLY: &str = "EGRESS_PROXY_REQUIRE_TERMINATION_FOR_NAME_ONLY";

/// Comma-separated host patterns relayed without termination.
pub const TLS_EXEMPT_HOSTS: &str = "EGRESS_PROXY_TLS_EXEMPT_HOSTS";

/// Comma-separated CIDR ranges relayed without termination.
pub const TLS_EXEMPT_ADDRESSES: &str = "EGRESS_PROXY_TLS_EXEMPT_ADDRESSES";

/// Every variable above, for a parent that starts the daemon with a cleared
/// environment and passes these through.
pub const FORWARDED_VARS: &[&str] = &[
    BINDING_TTL_SECS,
    REVALIDATE_INTERVAL_SECS,
    RESWEEP_LIVE_FLOWS,
    REQUIRE_TERMINATION_FOR_NAME_ONLY,
    TLS_EXEMPT_HOSTS,
    TLS_EXEMPT_ADDRESSES,
];
