use crate::effective::Effective;
use crate::enforcement_mode::EnforcementMode;
use crate::refusal::Refusal;
use crate::rule_ref::RuleRef;

/// The outcome of one evaluation.
///
/// Callers record [`Decision::consulted`] next to the request so a past decision
/// is reconstructable from the policy streams at the recorded revisions
/// (ADR#0071 Decision 7), and record [`Decision::would_deny`] in both modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    effective: Effective,
    would_deny: Option<Refusal>,
    mode: EnforcementMode,
    consulted: Vec<RuleRef>,
}

impl Decision {
    pub(crate) fn new(mode: EnforcementMode, refusal: Option<Refusal>, consulted: Vec<RuleRef>) -> Self {
        let effective = match (&refusal, mode) {
            (Some(refusal), EnforcementMode::Enforce) => Effective::Refuse(refusal.clone()),
            (Some(_), EnforcementMode::DryRun) | (None, _) => Effective::Admit,
        };
        Self {
            effective,
            would_deny: refusal,
            mode,
            consulted,
        }
    }

    /// What the caller does. In `DryRun` a refusal still yields `Admit`.
    pub fn effective(&self) -> &Effective {
        &self.effective
    }

    /// The refusal that happened, in both modes.
    pub fn would_deny(&self) -> Option<&Refusal> {
        self.would_deny.as_ref()
    }

    /// Every history that was read, with the revision it was read at, in a
    /// fixed order and without repeats. A history that did not exist is listed
    /// at revision zero.
    pub fn consulted(&self) -> &[RuleRef] {
        &self.consulted
    }

    /// The mode the evaluator ran in.
    pub fn mode(&self) -> EnforcementMode {
        self.mode
    }
}
