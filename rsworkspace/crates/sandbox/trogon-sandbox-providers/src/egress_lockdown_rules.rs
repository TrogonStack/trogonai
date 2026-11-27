/// The firewall commands that confine a sandbox's network namespace to TCP through its egress proxy, split by
/// address family so each can be installed and checked on its own.
///
/// Built by [`crate::egress_lockdown_rules`]. Each entry is one `iptables` or `ip6tables` invocation, in the order
/// it must be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressLockdownRules {
    /// The TCP redirect into the proxy, then the IPv4 `OUTPUT` filter ending in a catch-all reject.
    pub ipv4: Vec<String>,
    /// The IPv6 `OUTPUT` filter ending in a catch-all reject. There is no IPv6 redirect: the proxy listens on IPv4
    /// loopback only, so IPv6 egress is refused outright and clients fall back to IPv4.
    pub ipv6: Vec<String>,
}
