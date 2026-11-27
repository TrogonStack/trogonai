use trogon_sandbox_contract::{Host, InjectHosts, ResponseHeaderStrip};

use crate::pattern_matches;

/// The response headers to strip from replies of the hosts a binding injects at.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StripRule {
    pub hosts: InjectHosts,
    pub headers: ResponseHeaderStrip,
}

impl StripRule {
    /// Reports whether replies from `host:port` have their headers stripped by this rule.
    pub fn applies_at(&self, host: &Host, port: u16) -> bool {
        self.hosts
            .hosts()
            .iter()
            .any(|pattern| pattern_matches(pattern, host, port))
    }
}
