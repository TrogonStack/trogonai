use std::sync::Arc;

use trogon_sandbox_contract::{
    DomainOnlyAllows, ExternalProxy, Host, MitmUnixSocket, NetworkOptions, ResponseHeaderStrip, TlsExclusions,
    TlsInterceptionExemptions, UnlistedDestinations,
};

pub use crate::constants::DEFAULT_TUNNEL_CAP;
use crate::pattern_matches;

/// Reports whether a TLS exclusion covers the destination.
pub fn tls_exclusions_exclude(exclusions: &TlsExclusions, host: &Host, port: u16) -> bool {
    exclusions
        .as_slice()
        .iter()
        .any(|pattern| pattern_matches(pattern, host, port))
}

/// Reports whether the MITM socket handles the destination.
pub fn mitm_routes(mitm: &MitmUnixSocket, host: &Host, port: u16) -> bool {
    mitm.domains.iter().any(|pattern| pattern_matches(pattern, host, port))
}

/// Reports whether a hostname exemption names the destination.
pub fn tls_exemptions_name_host(exemptions: &TlsInterceptionExemptions, host: &Host, port: u16) -> bool {
    exemptions
        .hostnames()
        .iter()
        .any(|pattern| pattern_matches(pattern, host, port))
}

/// Reports whether both protocols are external, so the runtime starts no proxy of its own.
pub fn external_proxy_replaces_runtime_proxy(proxy: ExternalProxy) -> bool {
    proxy.http_port().is_some() && proxy.socks_port().is_some()
}

/// Reports whether a flow allowed by name alone is terminated so its `Host` header can be checked.
pub fn domain_only_allows_requires_termination(mode: DomainOnlyAllows) -> bool {
    mode == DomainOnlyAllows::RequireTermination
}

/// Reports whether a destination the policy does not list is refused without asking anyone.
pub fn unlisted_destinations_refuse(mode: UnlistedDestinations) -> bool {
    mode == UnlistedDestinations::Refuse
}

/// The response header strip to apply, or `None` when it strips nothing.
pub fn response_strip(options: &NetworkOptions) -> Option<Arc<ResponseHeaderStrip>> {
    (!options.strip_response_headers.is_empty()).then(|| Arc::new(options.strip_response_headers.clone()))
}
