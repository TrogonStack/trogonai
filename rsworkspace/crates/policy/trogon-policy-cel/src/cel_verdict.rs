use trogon_policy_eval::Narrowing;

use crate::cel_refusal::CelRefusal;

/// What the rule set says about one request.
///
/// There is no arm that permits. `Pass` means no rule refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CelVerdict {
    /// No rule refused the request.
    Pass,
    /// A rule refused it.
    Refused(CelRefusal),
}

impl CelVerdict {
    /// Drops the cause and keeps what the evaluator needs: whether to refuse
    /// and which rule to name.
    pub fn into_narrowing(self) -> Narrowing {
        match self {
            Self::Pass => Narrowing::Pass,
            Self::Refused(refusal) => Narrowing::Deny(refusal.rule().clone()),
        }
    }
}
