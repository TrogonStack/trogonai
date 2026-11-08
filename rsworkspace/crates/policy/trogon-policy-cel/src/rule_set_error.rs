use trogon_policy_eval::RuleName;

use crate::compile_error::CompileError;

/// Why a set of rules is not loaded. A set loads whole or not at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleSetError {
    /// Two rules share a name, so a refusal could not say which one it meant.
    #[error("two rules are named {name}")]
    Duplicate {
        /// The name used twice.
        name: RuleName,
    },
    /// The set holds more rules than the limit.
    #[error("the set holds {actual} rules, the limit is {limit}")]
    TooMany {
        /// How many rules were supplied.
        actual: usize,
        /// The most rules a set may hold.
        limit: usize,
    },
    /// One rule did not compile.
    #[error("rule {name} does not compile: {source}")]
    Rule {
        /// The rule that failed.
        name: RuleName,
        /// Why.
        source: CompileError,
    },
}
