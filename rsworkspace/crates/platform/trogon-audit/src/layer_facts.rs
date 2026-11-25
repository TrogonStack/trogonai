use crate::audit_name::AuditName;
use crate::consistency_token::ConsistencyToken;
use crate::decision_outcome::DecisionOutcome;
use crate::reason_code::ReasonCode;

/// One layer's verdict inside a decision (ADR#0084 reports one per layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerFacts {
    pub layer: AuditName,
    pub outcome: DecisionOutcome,
    pub reason: Option<ReasonCode>,
    pub rule_ids: Vec<AuditName>,
    /// The consistency token the layer was evaluated at, where it has one.
    pub consistency_token: Option<ConsistencyToken>,
}

impl LayerFacts {
    pub fn new(layer: AuditName, outcome: DecisionOutcome) -> Self {
        Self {
            layer,
            outcome,
            reason: None,
            rule_ids: Vec::new(),
            consistency_token: None,
        }
    }
}
