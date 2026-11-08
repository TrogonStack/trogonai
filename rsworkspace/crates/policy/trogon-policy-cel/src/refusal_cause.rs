use std::fmt;
use std::time::Duration;

use crate::facts_limit_error::FactsLimitError;
use crate::runtime_error_kind::RuntimeErrorKind;

/// Why a rule refused a request.
///
/// A rule that is false has refused as intended. Every other cause is a rule
/// that could not give an answer, and an answer that cannot be given is a
/// refusal, never a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalCause {
    /// The rule evaluated to `false`.
    False,
    /// The rule evaluated to something that is not a boolean.
    NotBoolean {
        /// The kind of value it produced.
        found: &'static str,
    },
    /// The interpreter failed while running the rule.
    Runtime(RuntimeErrorKind),
    /// The interpreter panicked while running the rule. The panic was caught
    /// and the request refused.
    Panicked,
    /// Evaluation took longer than the timeout.
    TimedOut {
        /// The timeout that was exceeded.
        limit: Duration,
    },
    /// The facts were larger than the input limits allow.
    FactsRejected(FactsLimitError),
}

impl RefusalCause {
    /// Returns whether the rule answered `false`, as opposed to failing to
    /// answer. A caller that raises an alert on a broken rule wants the other
    /// causes only.
    pub fn is_ordinary(&self) -> bool {
        matches!(self, Self::False)
    }
}

impl fmt::Display for RefusalCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::False => f.write_str("the rule evaluated to false"),
            Self::NotBoolean { found } => write!(f, "the rule evaluated to {found}, not a boolean"),
            Self::Runtime(kind) => write!(f, "the rule failed while running: {kind}"),
            Self::Panicked => f.write_str("the interpreter failed on the rule"),
            Self::TimedOut { limit } => write!(f, "evaluation took longer than {limit:?}"),
            Self::FactsRejected(error) => write!(f, "the facts were rejected: {error}"),
        }
    }
}
