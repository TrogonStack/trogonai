use serde::{Deserialize, Serialize};

use crate::MatchRule;

/// Request-forwarding rule shape for a domain rule.
///
/// A matched request (per `match_rules`, with the same OR-across-entries and empty-matches-all semantics as
/// [`CredentialTransform::match_rules`](crate::CredentialTransform::match_rules)) is forwarded to `forward_url`
/// instead of the connection's real (SNI-sniffed) destination, carrying an OIDC identity token scoped to
/// `forward_url` as audience.
///
/// It deliberately does not carry a tenant, lease, or sandbox identity. The struct is author-declared policy,
/// authored once and potentially reused across many leases, whereas tenant, lease, and identity are per-running-
/// process runtime context that the egress proxy supplies itself when it mints a token.
///
/// `ttl_seconds` bounds how long the OIDC token minted for a matched request is valid. It defaults to a short-lived
/// [`RequestForward::DEFAULT_TTL_SECONDS`] since tokens are minted fresh per forwarded request rather than cached.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestForward {
    pub forward_url: String,
    #[serde(default)]
    pub match_rules: Vec<MatchRule>,
    #[serde(default = "RequestForward::default_ttl_seconds")]
    pub ttl_seconds: u32,
}

impl RequestForward {
    /// Short default appropriate for a token minted fresh per forwarded request rather than cached or reused. It
    /// is long enough to survive the forwarded connection's handshake and request and response round trip, and
    /// short enough that a leaked token is useless shortly after.
    pub const DEFAULT_TTL_SECONDS: u32 = 60;

    pub fn default_ttl_seconds() -> u32 {
        Self::DEFAULT_TTL_SECONDS
    }
}
