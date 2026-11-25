use std::fmt;
use std::time::Duration;

use crate::evaluator_error::EvaluatorError;
use crate::hold_name::HoldName;
use crate::rule_kind::RuleKind;
use crate::rule_name::RuleName;
use crate::rule_ref::RuleRef;

/// Why a request was refused, naming exactly one binding.
///
/// When several bindings would refuse, the one reported is the smallest by the
/// derived order of this type. That order exists so the same facts always give
/// the same answer. It is not a precedence: no binding overrides another, and
/// every history that was read is still listed by `Decision::consulted`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Refusal {
    /// No permit of this kind on the dispatching Agent's own chain covers the
    /// action. Absence is denial.
    NoPermit(RuleKind),
    /// A hold on the lineage, or on the callee of a delegation, refuses.
    HoldPlaced {
        /// The hold history that contained it, at the revision read.
        rule: RuleRef,
        /// The one hold within that history.
        hold: HoldName,
    },
    /// A ceiling on the lineage excludes the action.
    CeilingExcludes(RuleRef),
    /// A request rule refused. It can only narrow.
    RequestRuleDenied(RuleName),
    /// The snapshot is older than the staleness budget.
    StaleSnapshot {
        /// How long ago the producer last vouched for the snapshot.
        age: Duration,
        /// The budget it exceeded.
        budget: Duration,
    },
    /// The evaluator could not make a decision.
    EvaluatorError(EvaluatorError),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPermit(kind) => write!(f, "no {kind} covers the request"),
            Self::HoldPlaced { rule, hold } => write!(f, "hold {hold} in the {rule} refuses the request"),
            Self::CeilingExcludes(rule) => write!(f, "the {rule} excludes the request"),
            Self::RequestRuleDenied(rule) => write!(f, "request rule {rule} refused the request"),
            Self::StaleSnapshot { age, budget } => {
                write!(f, "policy snapshot is {age:?} old, the budget is {budget:?}")
            }
            Self::EvaluatorError(error) => write!(f, "the evaluator could not decide: {error}"),
        }
    }
}
