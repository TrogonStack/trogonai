/// Why a rule is not loaded.
///
/// Every one of these is found when the rule is compiled, which is off the
/// request path, so a rule that would be refused on every request never ships.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// A rule with no expression has nothing to decide.
    #[error("rule source is empty")]
    Empty,
    /// The source is larger than the limit.
    #[error("rule source is {actual} bytes, the limit is {limit}")]
    SourceTooLong {
        /// The size supplied.
        actual: usize,
        /// The largest size accepted.
        limit: usize,
    },
    /// The expression nests deeper than the limit. The parser and the
    /// interpreter both recurse, and neither offers a depth limit of its own.
    #[error("rule nests deeper than {limit} levels")]
    TooDeep {
        /// The deepest nesting accepted.
        limit: usize,
    },
    /// The source is not an expression.
    #[error("rule does not parse: {message}")]
    Parse {
        /// What the parser reported.
        message: String,
    },
    /// The expression uses something the interpreter cannot evaluate.
    #[error("rule uses {construct}, which rules cannot")]
    Unsupported {
        /// What was used.
        construct: &'static str,
    },
    /// The most the rule can cost on facts within the input limits is above the
    /// cost limit.
    #[error("rule can cost {cost}, the limit is {limit}")]
    CostExceeded {
        /// The static upper bound on the cost, saturating.
        cost: u64,
        /// The cost limit.
        limit: u64,
    },
}
