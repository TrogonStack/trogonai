use std::time::Duration;

use crate::{
    AllowedTlsTermination, AmbiguousTargets, CaFiles, DeniedDomainReasons, DomainOnlyAllows, ExternalProxy,
    ExtraCaCertPaths, IpRange, LiveFlowResweep, LoopbackNames, MitmUnixSocket, NamesMatchedByName, ParentProxy,
    PlaintextHeaderSet, PrivateRangesByName, ReasonedRefusals, RequestBodyCap, ResponseHeaderStrip, TlsExclusions,
    TlsInterceptionExemptions, TunnelCap, UnlistedDestinations, UpstreamCaCertificates,
};

mod host_match;

pub use host_match::HostMatch;

/// Everything optional about the proxy's network behavior.
///
/// Every option is off by default, and none of them loosens the proxy's own defaults. Each field is valid on its
/// own, so they are public.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct NetworkOptions {
    pub ca_files: Option<CaFiles>,
    pub tls_exclusions: TlsExclusions,
    pub external_proxy: Option<ExternalProxy>,
    pub denied_domain_reasons: DeniedDomainReasons,
    pub unlisted: UnlistedDestinations,
    pub parent_proxy: Option<ParentProxy>,
    pub mitm: Option<MitmUnixSocket>,
    pub upstream_ca: UpstreamCaCertificates,
    pub extra_ca_cert_paths: ExtraCaCertPaths,
    pub tunnel_cap: Option<TunnelCap>,
    pub strip_response_headers: ResponseHeaderStrip,
    pub loopback_names: LoopbackNames,
    pub reasoned_refusals: ReasonedRefusals,
    pub terminate_allowed_tls: AllowedTlsTermination,
    pub plaintext_header_set: PlaintextHeaderSet,
    pub private_ranges_by_name: PrivateRangesByName,
    pub names_matched_by_name: NamesMatchedByName,
    pub denied_resolved_addresses: Vec<IpRange>,
    pub request_body_cap: Option<RequestBodyCap>,
    pub handshake_timeout: Option<Duration>,
    pub ambiguous_targets: AmbiguousTargets,
    /// Whether a request whose spelled target differs from where it was sent is refused.
    pub host_match: HostMatch,
    pub domain_only_allows: DomainOnlyAllows,
    pub resweep_live_flows: LiveFlowResweep,
    pub tls_interception_exemptions: TlsInterceptionExemptions,
    /// How long a name-to-address binding the proxy observed stays live. `None` keeps the built-in default.
    pub binding_ttl: Option<Duration>,
    /// How often open flows are revalidated against fresh resolution, apart from policy changes. `None` revalidates
    /// only on a policy change.
    pub revalidate_interval: Option<Duration>,
}
