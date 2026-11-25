use std::fmt;

use trogon_policy_eval::RuleName;

use crate::refusal_cause::RefusalCause;

/// The one rule that refused a request, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CelRefusal {
    rule: RuleName,
    cause: RefusalCause,
}

impl CelRefusal {
    pub(crate) fn new(rule: RuleName, cause: RefusalCause) -> Self {
        Self { rule, cause }
    }

    /// The rule that refused.
    pub fn rule(&self) -> &RuleName {
        &self.rule
    }

    /// Why it refused.
    pub fn cause(&self) -> &RefusalCause {
        &self.cause
    }
}

impl fmt::Display for CelRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rule {} refused: {}", self.rule, self.cause)
    }
}
