use std::time::Duration;

use a2a_nats::agent_id::A2aAgentId;
use trogon_audit::ReadOnly;
use trogon_identity_types::SubjectPrincipal;

use super::deny_reason::DenyReason;

/// What the gateway knows about one ingress decision.
///
/// Everything here is an identifier the subject already carries, a closed code,
/// a count or a duration. The payload is not here, not even by digest: the
/// params fingerprint of the legacy envelope is a confirmation oracle for
/// low-entropy params and is deliberately not carried (ADR#0086 Decision 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionEvent<'a> {
    /// The principal the ingress subject carried, never a header or a payload.
    pub caller: &'a SubjectPrincipal,
    pub agent_id: &'a A2aAgentId,
    /// The method in its `message/send` spelling.
    pub method: &'a str,
    pub read_only: ReadOnly,
    /// The gateway's trace id, a UUID.
    pub trace_id: &'a str,
    /// The JSON-RPC id of the request, when it is a plain string or number.
    pub request_id: Option<&'a str>,
    pub request_bytes: usize,
    pub latency: Duration,
    pub rules_fired: &'a [String],
    pub verdict: Verdict<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<'a> {
    Allowed(AllowedLayers<'a>),
    Denied(DenyReason),
}

/// Which layers took part in an allow, so the record can say what was actually
/// evaluated and not only that nothing refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllowedLayers<'a> {
    pub tier1_spicedb: bool,
    /// The SpiceDB consistency token the allow was evaluated at.
    pub tier1_consistency_token: Option<&'a str>,
    pub tier3: bool,
}
