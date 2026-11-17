use std::time::Duration;

use crate::audit_name::AuditName;
use crate::decision_outcome::DecisionOutcome;
use crate::layer_facts::LayerFacts;
use crate::read_only::ReadOnly;
use crate::reason_code::ReasonCode;
use crate::surface::Surface;
use crate::tls_mode::TlsMode;

/// What a gateway decided about one act. Every field is a name, a closed code, a
/// count or a duration: there is nowhere to put a header value, a body or a
/// prompt (ADR#0086 Decision 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionFacts {
    pub surface: Surface,
    /// The resource name, host or method acted on. A name, never a payload.
    pub target: AuditName,
    pub outcome: DecisionOutcome,
    /// The surface's own declaration for the method. It decides the class of an
    /// enforced allow and nothing else.
    pub read_only: ReadOnly,
    pub reason: Option<ReasonCode>,
    pub rule_ids: Vec<AuditName>,
    pub policy_source: Option<AuditName>,
    pub projection_revision: Option<u64>,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub latency: Option<Duration>,
    pub tls_mode: Option<TlsMode>,
    pub session: Option<AuditName>,
    pub layers: Vec<LayerFacts>,
}

impl DecisionFacts {
    pub fn new(surface: Surface, target: AuditName, outcome: DecisionOutcome) -> Self {
        Self {
            surface,
            target,
            outcome,
            read_only: ReadOnly::Undeclared,
            reason: None,
            rule_ids: Vec::new(),
            policy_source: None,
            projection_revision: None,
            request_bytes: None,
            response_bytes: None,
            latency: None,
            tls_mode: None,
            session: None,
            layers: Vec::new(),
        }
    }
}
