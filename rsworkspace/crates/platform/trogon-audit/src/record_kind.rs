//! The class a record gets (ADR#0086 Decision 3).
//!
//! The class is a function of what the record is, and nothing a caller passes
//! per call: a record is built from its facts and the class falls out of them.

use trogonai_proto::platform::audit::v1::AuditClass;

use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::obligation::Obligation;
use crate::read_only::ReadOnly;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Decision {
        outcome: DecisionOutcome,
        read_only: ReadOnly,
        break_glass: bool,
    },
    /// A release or an injection. Must-audit in every mode, because the effect
    /// happens whether or not a layer is in DryRun.
    CredentialUse,
    /// A policy or credential change. Must-audit in every mode.
    Change,
    /// Writer start and chain reset.
    Integrity,
}

impl RecordKind {
    pub fn audit_class(self) -> AuditClass {
        match self {
            Self::Decision {
                outcome,
                read_only,
                break_glass,
            } => decision_class(outcome, read_only, break_glass),
            Self::CredentialUse | Self::Change | Self::Integrity => AuditClass::Must,
        }
    }

    pub fn obligation(self) -> Obligation {
        let refused = matches!(self, Self::Decision { outcome, .. } if outcome.is_refusal());
        match (self.audit_class(), refused) {
            (AuditClass::BestEffort, _) => Obligation::BestEffort,
            (_, true) => Obligation::Stands,
            _ => Obligation::BeforeEffect,
        }
    }
}

/// Every DryRun record is best-effort, break-glass or not: nothing is enforced,
/// so a lost record changes no verdict. An enforced allow is must-audit unless the
/// surface declared the method read-only, and a break-glass actor's allow stays
/// must-audit even then.
fn decision_class(outcome: DecisionOutcome, read_only: ReadOnly, break_glass: bool) -> AuditClass {
    match (outcome.enforcement(), outcome) {
        (Enforcement::DryRun, _) => AuditClass::BestEffort,
        (Enforcement::Enforce, DecisionOutcome::Allow(_)) if read_only == ReadOnly::Declared && !break_glass => {
            AuditClass::BestEffort
        }
        (Enforcement::Enforce, _) => AuditClass::Must,
    }
}

#[cfg(test)]
mod tests;
